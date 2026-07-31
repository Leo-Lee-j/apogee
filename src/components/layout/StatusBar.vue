<script setup lang="ts">
import { computed } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { inTauri } from '@/ipc'

/** 状态栏，24px（02 文档 §3.2）。可在设置里隐藏。 */
const tabs = useTabsStore()
const dirtyCount = computed(() => tabs.tabs.filter((t) => t.dirty).length)
</script>

<template>
  <footer class="statusbar">
    <span class="ok">● 就绪</span>
    <span class="sep">|</span>
    <span>dev</span>
    <span class="sep">|</span>
    <span>代理: 系统</span>
    <span class="sep">|</span>
    <span v-if="dirtyCount">未保存 {{ dirtyCount }}</span>
    <span v-else class="dim">全部已保存</span>
    <span class="spacer" />
    <span class="dim">{{ inTauri ? 'Tauri' : '浏览器（Spike 对照）' }}</span>
    <span class="sep">|</span>
    <span class="dim">v0.1.0</span>
  </footer>
</template>

<style scoped>
.statusbar {
  height: var(--a-h-statusbar);
  flex: 0 0 auto;
  display: flex; align-items: center; gap: 8px;
  padding: 0 10px;
  background: var(--a-bg-2);
  border-top: 1px solid var(--a-border);
  font-size: 12px; color: var(--a-text-dim);
}
.ok { color: var(--a-ok); }
/* 分隔符是纯装饰，用 ghost 档；它不承载信息，按 WCAG 装饰元素豁免对比度。 */
.sep { color: var(--a-text-ghost); }
.dim { color: var(--a-text-faint); }
.spacer { flex: 1 1 auto; }
</style>
