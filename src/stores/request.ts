import {
  ipc,
  onRequestEvents,
  type RequestSpec,
  type ExecContext,
  type DoneEvent,
  type ErrorEvent,
  type CancelledEvent,
} from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { ref, onMounted, onUnmounted } from 'vue'

/**
 * 请求发送逻辑 ——「点发送 → 出结果」这条热路径的 MVC 粘合层。
 *
 * 为什么不是 tabs store 的一部分：tabs store 只管标签生命周期与状态，
 * 不管「send → listen → patch」这个异步链。放在这里测试时可以整体 mock。
 *
 * # 为什么监听在 setup 里注册一次，而不是每次 send 后注册
 *
 * 「sendRequest 返回句柄」与「注册监听」之间存在竞态：本地快服务可以在监听
 * 注册完成之前就把 request:done 发出来。Tauri 事件是 fire-and-forget，
 * 错过了就再也收不到 —— 表现就是「请求发出去了但响应区空白」。
 *
 * 所以监听只注册一次，事件里带 id，按 id 路由区分先后两次请求。
 */

export function useRequest() {
  const tabs = useTabsStore()
  const sending = ref(false)
  /** 当前在飞的请求：句柄 id → 它属于哪个标签。 */
  const active = ref<{ handle: string; tabId: string } | null>(null)

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

  onMounted(() => {
    unlisten = onRequestEvents({
      onDone(event: DoneEvent) {
        const current = active.value
        // 路由：只认当前在飞的这个请求的完成事件
        if (current && event.response.id === current.handle) {
          tabs.patch(current.tabId, { response: event.response, dirty: false })
        }
        sending.value = false
        active.value = null
        clearPendingTimeout()
      },
      onError(event: ErrorEvent) {
        // 后端已显式告知失败。按钮复位 + 控制台留痕。
        // 完整错误条(02 文档 §8 三段式)等响应面板落地后做。
        if (active.value && event.id === active.value.handle) {
          console.error('[request] 后端报告错误', event.error)
        }
        sending.value = false
        active.value = null
        clearPendingTimeout()
      },
      onCancelled(event: CancelledEvent) {
        if (active.value && event.id === active.value.handle) {
          // 取消后只需要复位按钮
        }
        sending.value = false
        active.value = null
        clearPendingTimeout()
      },
    })
  })

  onUnmounted(() => {
    unlisten?.()
    clearPendingTimeout()
  })

  /** 发送当前激活标签的请求。 */
  async function send() {
    const tab = tabs.active
    if (!tab || !tab.url.trim() || sending.value) return

    const spec: RequestSpec = {
      method: tab.method,
      url: tab.url,
      // 本切片只传方法 + URL。params/headers/body/auth 在各自的
      // 面板完整落地后由这里拼装 —— 现在传空等于发一个最简 GET/POST。
    }
    const ctx: ExecContext = { tabId: tab.id }

    sending.value = true
    armPendingTimeout()
    try {
      const handle = await ipc.sendRequest(spec, ctx)
      active.value = { handle: handle.id, tabId: tab.id }
    } catch (error) {
      // invoke 本身失败（命令注册问题等）
      console.error('[request] invoke sendRequest 失败', error)
      sending.value = false
      active.value = null
      clearPendingTimeout()
    }
  }

  /** 取消当前请求。 */
  function cancel() {
    if (active.value) void ipc.cancelRequest(active.value.handle)
    sending.value = false
    active.value = null
    clearPendingTimeout()
  }

  return { sending, send, cancel }
}
