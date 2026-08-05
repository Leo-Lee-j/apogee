import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

/** 布局模式（02 文档 §3.3）。`Ctrl+Shift+L` 循环。 */
export type LayoutMode = 'stacked' | 'side-by-side' | 'focus'
export type ThemeMode = 'dark' | 'light' | 'system'

/**
 * 侧边栏宽度区间（02 文档 §3.2：「370px，可拖拽 180～480」）。
 *
 * 上限不是随手定的：侧边栏是**导航**，主区才是工作面。没有上限时用户一拖到底，
 * URL 栏和响应区会被压到不可用，而这个状态还会被持久化下来 —— 下次启动直接
 * 打开一个坏掉的布局。180 是「GET + 一个中等长度接口名」不省略号的下界。
 */
export const SIDEBAR_MIN_W = 180
export const SIDEBAR_MAX_W = 480
export const SIDEBAR_DEFAULT_W = 370

/**
 * 主区的最小可用宽度。窗口窄到放不下「侧边栏上限 + 这个值」时，上限按窗口收紧。
 *
 * 硬上限 480 只在够宽的窗口里成立：1024 宽的窗口里给侧边栏 480，主区就只剩 540，
 * URL 栏那一行（方法选择器 84 + 发送 + 保存）会开始挤压地址输入框。
 */
const MAIN_MIN_W = 520

export const useUiStore = defineStore('ui', () => {
  const sidebarVisible = ref(true)
  const sidebarWidth = ref(SIDEBAR_DEFAULT_W)
  /** 视口宽度。由 AppBody 在 resize 时同步，用来把宽度上限收紧到窗口能承受的范围。 */
  const viewportW = ref(typeof window === 'undefined' ? 1280 : window.innerWidth)

  /**
   * 当前生效的宽度上限。
   *
   * 窗口极窄时 `viewportW - MAIN_MIN_W` 可能低于 `SIDEBAR_MIN_W`，此时下限优先 ——
   * 宁可让主区继续被压（它有 `min-width: 0`，会自己缩），也不能把侧边栏压到
   * 一列省略号。
   */
  const sidebarMaxW = computed(() =>
    Math.max(SIDEBAR_MIN_W, Math.min(SIDEBAR_MAX_W, viewportW.value - MAIN_MIN_W)),
  )

  /** 唯一的宽度写入口，一律夹在 [min, max] 内。拖拽、键盘、恢复配置都走这里。 */
  function setSidebarWidth(px: number) {
    if (!Number.isFinite(px)) return
    sidebarWidth.value = Math.round(Math.min(sidebarMaxW.value, Math.max(SIDEBAR_MIN_W, px)))
  }

  function resetSidebarWidth() {
    setSidebarWidth(SIDEBAR_DEFAULT_W)
  }

  /** 窗口尺寸变了：更新视口宽度并按新上限回夹一次当前宽度。 */
  function syncViewport(width: number) {
    viewportW.value = width
    setSidebarWidth(sidebarWidth.value)
  }

  const layout = ref<LayoutMode>('stacked')
  const themeMode = ref<ThemeMode>('light')
  /** 请求区占比，可拖拽。默认 45%（02 文档 §3.2）。 */
  const requestRatio = ref(0.45)

  const settingsOpen = ref(false)
  const historyOpen = ref(false)

  /**
   * 「把焦点交给 URL 栏」的信号计数器。
   *
   * 用自增数字而不是布尔：布尔要在消费方复位，两个组件互相写同一个状态；
   * 而且连续两次「新建请求」都要各自聚焦一次，布尔的 false→true→true 只会
   * 触发一次 watch。计数器天然幂等且可重复触发。
   *
   * 为什么不让 AppBody 持 UrlBar 的 ref 直接调 focus()：发起方可能是标签栏、命令面板、快捷键，
   * 全都不在 UrlBar 的父子链上（02 文档 §5.1「选完立刻进主界面并聚焦 URL 栏」）。信号量比到处传
   * ref 干净。
   */
  const urlFocusSeq = ref(0)
  function focusUrl() {
    urlFocusSeq.value++
  }
  const paletteOpen = ref(false)
  const spikeOpen = ref(false)
  /** 当前打开的 Spike 量测台。仅开发/量测用，不是产品功能。 */
  const spikePanel = ref<'s2' | 's3'>('s2')

  const systemDark = ref(
    typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches !== false,
  )
  const isDark = computed(() =>
    themeMode.value === 'system' ? systemDark.value : themeMode.value === 'dark',
  )

  function applyTheme() {
    if (typeof document === 'undefined') return
    document.documentElement.dataset.theme = isDark.value ? 'dark' : 'light'
  }

  function cycleLayout() {
    const order: LayoutMode[] = ['stacked', 'side-by-side', 'focus']
    layout.value = order[(order.indexOf(layout.value) + 1) % order.length]!
  }

  return {
    sidebarVisible, sidebarWidth, sidebarMaxW, layout, themeMode, requestRatio,
    settingsOpen, historyOpen, paletteOpen, spikeOpen, spikePanel,
    urlFocusSeq, focusUrl,
    systemDark, isDark, applyTheme, cycleLayout,
    setSidebarWidth, resetSidebarWidth, syncViewport,
  }
})
