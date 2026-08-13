import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

/** 布局模式（02 文档 §3.3）。`Ctrl+Shift+L` 循环。 */
export type LayoutMode = 'stacked' | 'side-by-side' | 'focus'
export type ThemeMode = 'dark' | 'light' | 'system'
export type LanguageMode = 'system' | 'zh-CN' | 'en-US'
export type FontFamilyMode = string
export type ShortcutId = 'command-palette' | 'settings' | 'new-request' | 'save-request' | 'toggle-sidebar' | 'cycle-layout' | 'send-request'

const SHORTCUT_PREFERENCES_KEY = 'apogee.shortcut-preferences'

function loadShortcutPreferences(): Partial<Record<ShortcutId, boolean>> {
  if (typeof localStorage === 'undefined') return {}
  try {
    return JSON.parse(localStorage.getItem(SHORTCUT_PREFERENCES_KEY) ?? '{}')
  } catch {
    return {}
  }
}

/**
 * 侧边栏宽度区间（02 文档 §3.2：「370px，可拖拽 180～480」）。
 *
 * 上限不是随手定的：侧边栏是**导航**，主区才是工作面。没有上限时用户一拖到底，
 * URL 栏和响应区会被压到不可用，而这个状态还会被持久化下来 —— 下次启动直接
 * 打开一个坏掉的布局。180 是「GET + 一个中等长度接口名」不省略号的下界。
 */
export const SIDEBAR_MIN_W = 180
export const SIDEBAR_MAX_W = 360
export const SIDEBAR_DEFAULT_W = 310

/**
 * 请求区 / 响应区的比例边界。
 *
 * 每个区域至少保留容器当前方向尺寸的约 1/3，最大约占 2/3；例如 900×600
 * 的容器中，左右布局的最小宽度约为 300px，上下布局的最小高度约为 200px。
 */
export const SPLIT_MIN_RATIO = 1 / 3
export const SPLIT_MAX_RATIO = 2 / 3
export const SPLIT_DEFAULT_RATIO = 0.4

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

  /** 默认左右分栏，让请求参数与响应结果在宽屏下并排显示。 */
  const layout = ref<LayoutMode>('side-by-side')
  const themeMode = ref<ThemeMode>('light')
  /** 请求区占比，可拖拽。默认 50%（02 文档 §3.2）。 */
  const requestRatio = ref(SPLIT_DEFAULT_RATIO)

  /** 所有分隔条入口统一夹在 1/3～2/3 内，保证每个区域保留至少约三分之一。 */
  function setRequestRatio(ratio: number) {
    if (!Number.isFinite(ratio)) return
    requestRatio.value = Math.min(SPLIT_MAX_RATIO, Math.max(SPLIT_MIN_RATIO, ratio))
  }

  function resetRequestRatio() {
    setRequestRatio(SPLIT_DEFAULT_RATIO)
  }

  const historyOpen = ref(false)
  /** 「保存到」位置选择器。草稿保存（保存按钮或 Ctrl+S）从任意位置打开它。 */
  const saveAsOpen = ref(false)
  /**
   * 设置页当前分区。命令面板 / Ctrl+, 深链会写这里；Settings 面板双向同步。
   * 放在 ui 而不是 tab 字段：设置是单例页，分区是导航状态不是标签身份。
   */
  const settingsSection = ref<'appearance' | 'startup' | 'network' | 'performance' | 'shortcuts' | 'about'>('appearance')

  /** 本地快捷键偏好。未记录的快捷键默认开启，避免升级后意外关闭已有操作。 */
  const shortcutPreferences = ref<Partial<Record<ShortcutId, boolean>>>(loadShortcutPreferences())
  function isShortcutEnabled(id: ShortcutId) {
    return shortcutPreferences.value[id] !== false
  }
  function setShortcutEnabled(id: ShortcutId, enabled: boolean) {
    shortcutPreferences.value = { ...shortcutPreferences.value, [id]: enabled }
    if (typeof localStorage !== 'undefined') {
      localStorage.setItem(SHORTCUT_PREFERENCES_KEY, JSON.stringify(shortcutPreferences.value))
    }
  }

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

  function applyFont(font: FontFamilyMode) {
    if (typeof document === 'undefined') return
    const root = document.documentElement
    const uiFonts: Record<string, string> = {
      system: "'Inter', 'Segoe UI', -apple-system, BlinkMacSystemFont, 'PingFang SC', 'Microsoft YaHei UI', sans-serif",
      inter: "'Inter', 'Segoe UI', sans-serif",
      segoe: "'Segoe UI', 'Inter', sans-serif",
      yahei: "'Microsoft YaHei UI', 'Microsoft YaHei', 'PingFang SC', sans-serif",
    }
    const legacyAliases: Record<string, string> = { inter: 'Inter', segoe: 'Segoe UI', yahei: 'Microsoft YaHei UI' }
    const requested = legacyAliases[font] ?? font
    const family = uiFonts[requested] ?? (requested === 'system' ? uiFonts.system : `'${requested.replace(/["']/g, '')}', ${uiFonts.system}`)
    root.style.setProperty('--a-font-ui', family)
  }

  /**
   * 请求编辑区的独立字体。默认沿用原本的等宽栈，使既有配置视觉上零变化；
   * 仅在用户显式选择时覆写，不影响响应查看器的 --a-font-mono。
   */
  function applyEditorFont(font: FontFamilyMode) {
    if (typeof document === 'undefined') return
    const root = document.documentElement
    const legacyAliases: Record<string, string> = { inter: 'Inter', segoe: 'Segoe UI', yahei: 'Microsoft YaHei UI' }
    const requested = (legacyAliases[font] ?? font).trim()
    const family = !requested || requested === 'system'
      ? 'var(--a-font-mono)'
      : `'${requested.replace(/["']/g, '')}', var(--a-font-mono)`
    root.style.setProperty('--a-font-editor', family)
  }

  function applyFontSizes(uiSize: number, resultSize: number) {
    if (typeof document === 'undefined') return
    const root = document.documentElement
    const boundedUiSize = Math.min(24, Math.max(11, Math.round(uiSize)))
    const boundedResultSize = Math.min(24, Math.max(11, Math.round(resultSize)))
    root.style.setProperty('--a-fs-base', `${boundedUiSize}px`)
    root.style.setProperty('--a-fs-sm', `${Math.max(10, boundedUiSize - 1)}px`)
    root.style.setProperty('--a-fs-xs', `${Math.max(9, boundedUiSize - 2)}px`)
    root.style.setProperty('--a-font-result', `${boundedResultSize}px`)
    root.style.setProperty('--a-lh-result', `${Math.round(boundedResultSize * 1.54)}px`)
  }

  function cycleLayout() {
    const order: LayoutMode[] = ['side-by-side', 'stacked', 'focus']
    const index = order.indexOf(layout.value)
    setLayout(order[(index < 0 ? 0 : index + 1) % order.length]!)
  }

  function setLayout(next: LayoutMode) {
    if (layout.value === next) return
    layout.value = next
    resetRequestRatio()
  }

  return {
    sidebarVisible, sidebarWidth, sidebarMaxW, layout, themeMode, requestRatio,
    historyOpen, saveAsOpen, settingsSection, shortcutPreferences, isShortcutEnabled, setShortcutEnabled, paletteOpen, spikeOpen, spikePanel,
    urlFocusSeq, focusUrl,
    systemDark, isDark, applyTheme, applyFont, applyEditorFont, applyFontSizes, cycleLayout, setLayout,
    setSidebarWidth, resetSidebarWidth, syncViewport,
    setRequestRatio, resetRequestRatio,
  }
})
