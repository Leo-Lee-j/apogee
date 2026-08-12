<script setup lang="ts">
import { computed } from 'vue'
import { NConfigProvider, darkTheme, lightTheme, zhCN, enUS } from 'naive-ui'
import { useUiStore } from '@/stores/ui'
import { darkThemeOverrides, lightThemeOverrides } from '@/theme'
import { useAppStore } from '@/stores/app'

/**
 * Naive UI 的懒加载主题边界。
 *
 * 默认变体由 AppShellNaive 在根节点提供 ConfigProvider；这里直接透传，避免
 * 嵌套 provider 重复注入样式。S5 变体 B 没有根 provider，所以只在该变体的
 * 动态模块里补一个本地 provider，保证 NInput/NModal 仍使用 Apogee 主题。
 */
const ui = useUiStore()
const app = useAppStore()
const localProvider = __S5_VARIANT__ === 'b'
const theme = computed(() => (ui.isDark ? darkTheme : lightTheme))
const overrides = computed(() => (ui.isDark ? darkThemeOverrides : lightThemeOverrides))
const locale = computed(() => app.settings.language === 'en-US' ? enUS : zhCN)
</script>

<template>
  <NConfigProvider
    v-if="localProvider"
    :theme="theme"
    :theme-overrides="overrides"
    :locale="locale"
    abstract
  >
    <slot />
  </NConfigProvider>
  <slot v-else />
</template>
