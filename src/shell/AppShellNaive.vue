<script setup lang="ts">
import { computed } from 'vue'
import { NConfigProvider, NMessageProvider, NDialogProvider, darkTheme, lightTheme } from 'naive-ui'
import AppBody from './AppBody.vue'
import { useUiStore } from '@/stores/ui'
import { darkThemeOverrides, lightThemeOverrides } from '@/theme'

/**
 * Spike S5 变体 A —— **按 02 文档 §7.5 原样实现**。
 *
 * 「`n-config-provider` 只包一层，放在应用根节点」。
 * 这么做的代价是 naive-ui 的运行时（含 css-render 这套 CSS-in-JS 引擎）
 * 被钉死在首屏 chunk 里，无法懒加载。
 *
 * S5 就是来量这笔开销够不够 150KB gzip 预算的。
 *
 * 注意 themeOverrides 直接引用模块级常量，`computed` 里只做「选哪个常量」，
 * 不重建对象 —— 02 文档 §7.5 第 2 条。
 */

const ui = useUiStore()
const theme = computed(() => (ui.isDark ? darkTheme : lightTheme))
const overrides = computed(() => (ui.isDark ? darkThemeOverrides : lightThemeOverrides))
</script>

<template>
  <NConfigProvider :theme="theme" :theme-overrides="overrides" abstract>
    <NDialogProvider>
      <NMessageProvider>
        <AppBody />
      </NMessageProvider>
    </NDialogProvider>
  </NConfigProvider>
</template>
