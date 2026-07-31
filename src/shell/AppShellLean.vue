<script setup lang="ts">
import AppBody from './AppBody.vue'

/**
 * Spike S5 变体 B —— 对照方案：首屏**零 naive-ui**。
 *
 * 首屏完全靠 CSS 变量 + 自研组件（见 src/styles/tokens.css）。
 * naive-ui 只在动态 import 的模块里出现（目前是设置页），
 * 因此整个 vendor-naive chunk 变成按需下载。
 *
 * 代价：
 *   - 用不了 `n-config-provider` 的全局主题，naive-ui 组件的主题需要在
 *     各个懒加载入口各自包一层 ConfigProvider（多个 provider 会各自注入样式，
 *     需要确认是否产生重复样式）
 *   - `useMessage()` / `useDialog()` 这类依赖 provider 的 composable 在首屏不可用，
 *     Toast 得自研
 *
 * 这两笔代价值不值得，取决于变体 A 到底超预算多少 —— 这就是 S5 要给的数字。
 */
</script>

<template>
  <AppBody />
</template>
