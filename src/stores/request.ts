import {
  ipc,
  inTauri,
  normalizeError,
  onRequestEvents,
  toHistoryRequestPreview,
  type RequestSpec,
  type ExecContext,
  type AppError,
  type DoneEvent,
  type ErrorEvent,
  type CancelledEvent,
  type WorkspaceSession,
} from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import { ref, onMounted, onUnmounted } from 'vue'

/**
 * 请求发送逻辑 ——「点发送 → 出结果」这条热路径的 MVC 粘合层。
 *
 * 为什么不是 tabs store 的一部分：tabs store 只管标签生命周期与状态，
 * 不管「send → listen → patch」这个异步链。放在这里测试时可以整体 mock。
 *
 * # 两个竞态，都出在「事件可能比句柄先到」
 *
 * `send_request` 立即返回句柄，真正的执行在 Rust 侧的后台任务里。对着 localhost
 * 发请求时，那个任务可以在 `invoke` 的返回值走完 IPC 回到 JS **之前**就把
 * `request:done` 发出来。于是：
 *
 * 1. 监听必须在 setup 里注册一次，不能等 send 之后再注册 —— Tauri 事件是
 *    fire-and-forget，错过就再也收不到。
 * 2. 光注册早还不够。**按 id 路由的那张表本身也可能还没填。** 事件到达时
 *    `active` 仍是 null，一个 `if (active && ...)` 的守卫会把响应直接丢掉，
 *    症状是「请求发出去了、按钮复位了、响应区空白」。所以认不出主人的终结事件
 *    先存进 `orphans`，等 `send()` 拿到句柄再认领。
 *
 * 另一头同样重要：状态复位必须**按 id 收敛**。取消一个请求后立刻发下一个时，
 * 上一个请求的迟到事件会跟着到 —— 无条件复位 `sending` 会把新请求的「等待中」
 * 提前抹掉，按钮变回「发送」而响应还在路上。
 */

/** 一个请求的终结状态。三者在 UI 上的处理不同，但复位逻辑相同。 */
type Settlement =
  | { kind: 'done'; event: DoneEvent }
  | { kind: 'error'; event: ErrorEvent }
  | { kind: 'cancelled'; event: CancelledEvent }

function eventSession(settlement: Settlement): WorkspaceSession {
  return {
    workspaceId: settlement.event.workspaceId,
    sessionId: settlement.event.sessionId,
  }
}

function sameSession(left: WorkspaceSession, right: WorkspaceSession): boolean {
  return left.workspaceId === right.workspaceId && left.sessionId === right.sessionId
}

/**
 * 早到/迟到事件的缓存条数上限。
 *
 * 正常情况下最多 1 条（当前请求的事件抢在句柄前面）。设上限是因为迟到事件的
 * 主人可能永远不来认领（标签已关、组件已卸载），无界的 Map 就是内存泄漏。
 */
const MAX_ORPHANS = 8

export function useRequest() {
  const tabs = useTabsStore()
  const workspace = useWorkspaceStore()
  const sending = ref(false)
  /** 当前在飞的请求：句柄 id → 它属于哪个标签。 */
  const active = ref<{
    handle: string
    tabId: string
    session: WorkspaceSession
    spec: RequestSpec
    startedAt: number
    debugCaseId?: string
    environmentId?: string
  } | null>(null)
  /** 还没找到主人的终结事件，按句柄 id 存。见文件头注释的竞态 2。 */
  const orphans = new Map<string, Settlement>()

  function currentSession(): WorkspaceSession | null {
    const workspaceId = workspace.workspaceId
    const sessionId = workspace.sessionId
    return workspaceId && sessionId ? { workspaceId, sessionId } : null
  }

  function isCurrentSession(session: WorkspaceSession): boolean {
    const current = currentSession()
    return current !== null && sameSession(current, session)
  }

  let unlisten: (() => void) | undefined
  /** 兜底超时句柄：防止后端任务崩溃/丢失事件导致按钮永远卡在「等待中」。 */
  let pendingTimeout: ReturnType<typeof setTimeout> | undefined

  function clearPendingTimeout() {
    if (pendingTimeout !== undefined) {
      clearTimeout(pendingTimeout)
      pendingTimeout = undefined
    }
  }

  function armPendingTimeout() {
    clearPendingTimeout()
    // 90s 远大于任何正常 HTTP 请求。后端在 panic 兜底下也会发
    // request:error,所以这只是「后端完全沉默」的最后防线。
    pendingTimeout = setTimeout(() => {
      if (sending.value) {
        console.warn('[request] 后端 90s 内未发出任何终结事件,主动复位发送状态')
        sending.value = false
        active.value = null
      }
    }, 90_000)
  }

  /** 把一个终结状态落到 UI 上，并复位发送状态。 */
  function apply(
    tabId: string,
    settlement: Settlement,
    session: WorkspaceSession,
    spec?: RequestSpec,
    startedAt?: number,
    debugCaseId?: string,
    environmentId?: string,
  ) {
    if (settlement.kind === 'done') {
      tabs.patch(tabId, {
        response: settlement.event.response,
        error: null,
        actualRequest: null,
        dirty: false,
      })
      if (inTauri && spec) {
        void ipc.historyRecord(
          startedAt ?? Date.now(),
          toHistoryRequestPreview(spec),
          settlement.event.response,
          debugCaseId,
          environmentId,
          session,
        )
          .catch((error) => console.error('[history] 保存请求历史失败', error))
      }
    } else if (settlement.kind === 'error') {
      // 网络失败也保留后端在发送前构造的最终请求，供 ResponsePanel 显示。
      const errorEvent = settlement.event
      tabs.patch(tabId, {
        response: null,
        error: errorEvent.error,
        actualRequest: errorEvent.actualRequest ?? null,
      })
      // 错误运行同样是调试用例的结果。它没有响应体，因此写入专门的失败历史记录，
      // 保留错误、实际请求和耗时，而不是伪造一个空 ResponseSummary。
      if (inTauri && spec) {
        const completedAt = Date.now()
        const createdAtMs = startedAt ?? completedAt
        void ipc.historyRecordError(
          createdAtMs,
          toHistoryRequestPreview(spec),
          errorEvent.error,
          errorEvent.actualRequest ?? null,
          Math.max(0, completedAt - createdAtMs),
          debugCaseId,
          environmentId,
          session,
        )
          .catch((error) => console.error('[history] 保存失败请求历史失败', error))
      }
    }
    sending.value = false
    active.value = null
    clearPendingTimeout()
  }

  /**
   * 收到一个终结事件。认得出主人就立刻处理，认不出就存着。
   *
   * 「认不出」有两种：句柄还没回到前端（竞态 2），或者这是一条已经被放弃的
   * 旧请求的迟到事件。两者都**不能**碰当前的 `sending`。
   */
  function settle(id: string, settlement: Settlement) {
    const session = eventSession(settlement)
    // Tauri 事件可能在 Rust 已切换 workspace 后才送达 WebView。只要事件的
    // originating session 不再是当前 session，就既不写 UI，也不缓存为 orphan。
    if (!isCurrentSession(session)) return

    const current = active.value
    if (current && current.handle === id && sameSession(current.session, session)) {
      apply(
        current.tabId,
        settlement,
        session,
        current.spec,
        current.startedAt,
        current.debugCaseId,
        current.environmentId,
      )
      return
    }
    if (orphans.size >= MAX_ORPHANS) {
      const oldest = orphans.keys().next().value
      if (oldest !== undefined) orphans.delete(oldest)
    }
    orphans.set(id, settlement)
  }

  onMounted(() => {
    unlisten = onRequestEvents({
      onDone: (event) => settle(event.response.id, { kind: 'done', event }),
      onError: (event) => settle(event.id, { kind: 'error', event }),
      onCancelled: (event) => settle(event.id, { kind: 'cancelled', event }),
    })
  })

  onUnmounted(() => {
    unlisten?.()
    clearPendingTimeout()
    orphans.clear()
  })

  /** 发送当前激活标签的请求。 */
  async function send() {
    const tab = tabs.active
    // History opens a deliberately non-executable projection. Keep this guard at the execution
    // entry point so keyboard shortcuts or future callers cannot bypass a disabled UI control.
    if (!tab || tab.historyPreview || !tab.url.trim() || sending.value) return

    // 工作区是首屏之后异步加载的。发送必须等它完成，否则默认环境还没
    // 选出来时会把带 {{variable}} 的请求误判成缺少变量。
    if (workspace.available && !workspace.ready) {
      await workspace.load()
      if (!workspace.ready) {
        const error: AppError = workspace.lastError ?? {
          kind: 'io',
          message: '工作区尚未加载，无法发送请求',
          hints: [{ label: '检查工作区目录是否可读，并重试' }],
        }
        tabs.patch(tab.id, { response: null, error, actualRequest: null })
        return
      }
    }

    const spec: RequestSpec = {
      method: tab.method,
      url: tab.url,
      params: tab.params,
      pathParams: tab.pathParams,
      headers: tab.headers,
      body: tab.body,
      auth: tab.auth,
      settings: tab.settings,
    }
    const startedAt = Date.now()
    // 将运行关联和环境在发送前固定下来。发送期间切换环境不能改写本次用例结果归属。
    const debugCaseId = tab.debugCaseId
    const environmentId = workspace.activeEnvironmentId ?? undefined

    // 新请求开始后清除上一次错误和快照；旧响应先保留，直到新请求成功或失败。
    tabs.patch(tab.id, { error: null, actualRequest: null })

    sending.value = true
    armPendingTimeout()
    try {
      // 环境编辑器采用显式保存按钮，但发送不能读到旧磁盘快照；把所有
      // 尚未保存的环境先 flush，再只传环境 id 给 Rust。
      if (!(await workspace.saveDirtyEnvironments())) {
        const error: AppError = workspace.lastError ?? {
          kind: 'io',
          message: '环境保存失败，未发送请求',
          hints: [{ label: '检查环境名称和工作区目录后重试' }],
        }
        tabs.patch(tab.id, { response: null, error, actualRequest: null })
        sending.value = false
        active.value = null
        clearPendingTimeout()
        return
      }

      const requestSession = currentSession()
      if (!requestSession) {
        // 工作区在保存环境期间已关闭或切换；不能再让默认 sessionArgs() 误把这次
        // 发送绑定到新 workspace。
        sending.value = false
        active.value = null
        clearPendingTimeout()
        return
      }

      const ctx: ExecContext = {
        tabId: tab.id,
        environmentId,
        // 用例标签不具备可写 nodeId，仍需其来源请求来解析集合/文件夹继承。
        requestNodeId: tab.nodeId ?? tab.sourceRequestId,
      }
      const handle = await ipc.sendRequest(spec, ctx, requestSession)
      const early = orphans.get(handle.id)
      if (!isCurrentSession(requestSession)) {
        // RPC 句柄可能在工作区切换后才回到前端；它及其早到事件不属于新 UI。
        orphans.delete(handle.id)
        sending.value = false
        active.value = null
        clearPendingTimeout()
      } else if (early && sameSession(eventSession(early), requestSession)) {
        // 事件跑在句柄前面了，在这里补做。
        orphans.delete(handle.id)
        apply(tab.id, early, requestSession, spec, startedAt, debugCaseId, environmentId)
      } else {
        // 即便 UUID 理论上不会复用，也不能把不同 session 的 orphan 留给本次请求。
        orphans.delete(handle.id)
        active.value = {
          handle: handle.id,
          tabId: tab.id,
          session: requestSession,
          spec,
          startedAt,
          debugCaseId,
          environmentId,
        }
      }
    } catch (error) {
      // invoke 本身失败（命令注册问题等）也进入同一套错误展示。
      const appError = normalizeError(error)
      tabs.patch(tab.id, { response: null, error: appError, actualRequest: null })
      console.error('[request] invoke sendRequest 失败', appError)
      sending.value = false
      active.value = null
      clearPendingTimeout()
    }
  }

  /**
   * 取消当前请求。
   *
   * 按钮立刻复位，不等后端确认 —— 取消是用户意图明确的动作，等一次往返只会
   * 让界面显得没反应。随后到达的 `request:cancelled` 会因为 id 已不匹配而落进
   * `orphans`，不会干扰下一个请求。
   */
  function cancel() {
    if (active.value) void ipc.cancelRequest(active.value.handle)
    sending.value = false
    active.value = null
    clearPendingTimeout()
  }

  return { sending, send, cancel }
}
