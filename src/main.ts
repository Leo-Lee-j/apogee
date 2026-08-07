import { createApp } from 'vue'
import { createPinia } from 'pinia'
import './styles/tokens.css'
import AppShell from '@app-shell'
import { useUiStore } from './stores/ui'
import { useAppStore } from './stores/app'
import { ipc, inTauri, type BootMetrics } from './ipc'

/**
 * 分阶段启动（03 文档 §4.1）。
 *
 * 目标：冷启动 < 800ms 到「可输入 URL 并发送」。
 *
 * 第一阶段只挂载 shell（顶栏 + URL 栏 + 空响应区）并聚焦输入框。
 * 侧边栏、历史、设置、命令面板、CodeMirror 全部走动态 import，
 * 不进首屏 chunk。
 *
 * 这里刻意**不做**的事：
 *   - 不读集合内容（只读 manifest 索引，且延后到侧边栏挂载时）
 *   - 不建 SQLite 连接、不起脚本引擎、不装文件监听
 *   - 不发任何网络请求（更新检查延后到启动 30s 后，且可关闭）
 */

const t0 = performance.now()

const app = createApp(AppShell)
app.use(createPinia())

// Pinia 必须在 mount 之前装好，store 才能在 setup 里用
const bootMarks: Partial<BootMetrics> = {}

app.mount('#app')
bootMarks.mountMs = performance.now() - t0

// 主题在挂载后立刻应用：CSS 变量切换是同步的，不会有闪烁
const ui = useUiStore()
ui.applyTheme()

// 入口首帧只加载轻量工作区注册表。只有用户选择工作区后才挂载请求编辑器。
if (inTauri) void useAppStore().initialize()

bootMarks.readyMs = performance.now() - t0

// 首帧真正画完之后再让 Rust 显示窗口（03 文档 §4.1）。
// 用双 rAF 而不是 nextTick：nextTick 只保证 Vue 更新完，像素还没上屏。
if (inTauri) {
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      bootMarks.firstPaintMs = performance.now() - t0
      void ipc.appReady({
        mountMs: bootMarks.mountMs!,
        readyMs: bootMarks.readyMs!,
        firstPaintMs: bootMarks.firstPaintMs,
      }).catch(() => {
        /* 兜底逻辑在 Rust 侧（3s 强制显示），这里失败也不阻塞 */
      })
    }),
  )
}

// 暴露启动量测，便于 Spike 与「性能自检」面板读取（PRD §5.2）
;(window as unknown as { __apogeeBoot: unknown }).__apogeeBoot = bootMarks

// eslint-disable-next-line no-console
console.info(
  `[Apogee] 变体=${__S5_VARIANT__} 挂载=${bootMarks.mountMs!.toFixed(1)}ms 就绪=${bootMarks.readyMs!.toFixed(1)}ms`,
)
