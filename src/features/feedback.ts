import { createDiscreteApi, darkTheme, lightTheme } from 'naive-ui'
import { darkThemeOverrides, lightThemeOverrides } from '@/theme'
import { useUiStore } from '@/stores/ui'

/**
 * 无 provider 依赖的 naive-ui message。
 *
 * 不用 `useMessage()`：它依赖注入树上的 NMessageProvider，而 S5 变体 b
 * （首屏零 naive-ui）没有根 provider，在首屏组件（UrlBar）里调用会直接抛错。
 * discrete 模式自带独立容器，两个变体都能用。
 *
 * 每次调用新建实例以跟随当前明暗主题 —— discrete 实例创建后不响应主题切换，
 * 而保存这类反馈是低频用户动作，实例重建的开销可以忽略。
 */
export function toast() {
  const ui = useUiStore()
  return createDiscreteApi(['message'], {
    configProviderProps: {
      theme: ui.isDark ? darkTheme : lightTheme,
      themeOverrides: ui.isDark ? darkThemeOverrides : lightThemeOverrides,
    },
  }).message
}

/** 把 AppError 的 message + hints 拼成一条 toast 文案，hints 保证可操作建议可见。 */
export function errorText(message: string, hints: { label: string }[] = []): string {
  if (hints.length === 0) return message
  return `${message}（${hints.map((hint) => hint.label).join('；')}）`
}
