import {
  ipc,
  normalizeError,
  onRequestEvents,
  type RequestSpec,
  type ExecContext,
  type DoneEvent,
  type ErrorEvent,
} from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
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
  | { kind: 'cancelled' }

/**
 * 早到/迟到事件的缓存条数上限。
 *
 * 正常情况下最多 1 条（当前请求的事件抢在句柄前面）。设上限是因为迟到事件的
 * 主人可能永远不来认领（标签已关、组件已卸载），无界的 Map 就是内存泄漏。
 */
const MAX_ORPHANS = 8

export function useRequest() {
  const tabs = useTabsStore()
  const sending = ref(false)
  /** 当前在飞的请求：句柄 id → 它属于哪个标签。 */
  const active = ref<{ handle: string; tabId: string } | null>(null)
  /** 还没找到主人的终结事件，按句柄 id 存。见文件头注释的竞态 2。 */
  const orphans = new Map<string, Settlement>()

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
  function apply(tabId: string, settlement: Settlement) {
    if (settlement.kind === 'done') {
      tabs.patch(tabId, {
        response: settlement.event.response,
        error: null,
        dirty: false,
      })
    } else if (settlement.kind === 'error') {
      // 错误进入 Tab 状态，由 ResponsePanel 渲染三段式错误，而不是只留在控制台。
      tabs.patch(tabId, { response: null, error: settlement.event.error })
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
    const current = active.value
    if (current && current.handle === id) {
      apply(current.tabId, settlement)
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
      onCancelled: (event) => settle(event.id, { kind: 'cancelled' }),
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
    if (!tab || !tab.url.trim() || sending.value) return

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
    const ctx: ExecContext = { tabId: tab.id }

    // 新请求开始后清除上一次错误；旧响应先保留，直到新请求成功或失败。
    tabs.patch(tab.id, { error: null })

    sending.value = true
    armPendingTimeout()
    try {
      const handle = await ipc.sendRequest(spec, ctx)
      const early = orphans.get(handle.id)
      if (early) {
        // 事件跑在句柄前面了，在这里补做。
        orphans.delete(handle.id)
        apply(tab.id, early)
      } else {
        active.value = { handle: handle.id, tabId: tab.id }
      }
    } catch (error) {
      // invoke 本身失败（命令注册问题等）也进入同一套错误展示。
      const appError = normalizeError(error)
      tabs.patch(tab.id, { response: null, error: appError })
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
