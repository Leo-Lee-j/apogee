import type { AppError } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
// 注意：不要静态 import feedback —— 它带 naive-ui message 运行时，
// 会破坏 S5 变体 b 的「首屏零 naive-ui」约束，保持动态 import。

/**
 * 保存当前标签的统一入口：UrlBar 的「保存」按钮和全局 Ctrl/Cmd+S 都走这里。
 *
 * 成功/失败统一在这里 toast 反馈，保证按钮与快捷键行为一致；草稿不 toast，
 * 由「保存到」选择器在确认成功后 toast。
 *
 * 三种结果：
 * - `saved`：已绑定集合节点的标签，内容已落盘。
 * - `needs-target`：草稿标签，没有可写盘的节点——打开「保存到」选择器建节点。
 * - `error`：非请求标签，或落盘失败。`error` 字段携带归一化 AppError（含 hints）。
 */
export type SaveRequestResult =
  | { status: 'saved' }
  | { status: 'needs-target' }
  | { status: 'error'; error?: AppError }

export async function saveCurrentRequest(): Promise<SaveRequestResult> {
  const tabs = useTabsStore()
  const ui = useUiStore()
  const tab = tabs.active
  if (!tab || tab.kind !== 'request') return { status: 'error' }
  if (!tab.nodeId) {
    ui.saveAsOpen = true
    return { status: 'needs-target' }
  }
  const result = await tabs.save(tab.id)
  if (result.ok) {
    const { toast } = await import('@/features/feedback')
    toast().success('已保存')
    return { status: 'saved' }
  }
  const error = result.error
  const { errorText, toast } = await import('@/features/feedback')
  toast().error(errorText(error?.message ?? '保存失败', error?.hints))
  return { status: 'error', error: error ?? undefined }
}
