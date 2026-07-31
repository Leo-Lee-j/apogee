import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

/** 布局模式（02 文档 §3.3）。`Ctrl+Shift+L` 循环。 */
export type LayoutMode = 'stacked' | 'side-by-side' | 'focus'
export type ThemeMode = 'dark' | 'light' | 'system'

export const useUiStore = defineStore('ui', () => {
  const sidebarVisible = ref(true)
  const sidebarWidth = ref(240)
  const layout = ref<LayoutMode>('stacked')
  const themeMode = ref<ThemeMode>('dark')
  /** 请求区占比，可拖拽。默认 45%（02 文档 §3.2）。 */
  const requestRatio = ref(0.45)

  const settingsOpen = ref(false)
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
    sidebarVisible, sidebarWidth, layout, themeMode, requestRatio,
    settingsOpen, paletteOpen, spikeOpen, spikePanel,
    systemDark, isDark, applyTheme, cycleLayout,
  }
})
