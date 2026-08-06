<script setup lang="ts">
import { computed } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { inTauri } from '@/ipc'

/** 状态栏，24px（02 文档 §3.2）。可在设置里隐藏。 */
const tabs = useTabsStore()
const ui = useUiStore()
const dirtyCount = computed(() => tabs.tabs.filter((t) => t.dirty).length)
const layoutLabel = computed(() => ({
  stacked: '上下分栏',
  'side-by-side': '左右分栏',
  focus: '专注模式',
}[ui.layout]))
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
    <button
      class="layout-toggle"
      type="button"
      :aria-label="`切换内容区布局，当前：${layoutLabel}`"
      :title="`${layoutLabel}，点击切换布局`"
      @click="ui.cycleLayout"
    >
      <svg v-if="ui.layout === 'stacked'" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="3" width="18" height="7" rx="1" />
        <rect x="3" y="14" width="18" height="7" rx="1" />
      </svg>
      <svg v-else-if="ui.layout === 'side-by-side'" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="3" width="7" height="18" rx="1" />
        <rect x="14" y="3" width="7" height="18" rx="1" />
      </svg>
      <svg v-else viewBox="0 0 24 24" aria-hidden="true">
        <path d="M8 3H3v5M16 3h5v5M8 21H3v-5M16 21h5v-5" />
      </svg>
    </button>
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
.layout-toggle {
  width: 26px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-left: 2px;
  padding: 0;
  border: 1px solid transparent;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text-faint);
  cursor: pointer;
}
.layout-toggle:hover { background: var(--a-hover); color: var(--a-text); }
.layout-toggle:focus-visible { outline: 2px solid var(--a-focus); outline-offset: 1px; }
.layout-toggle svg {
  width: 15px;
  height: 15px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}
</style>
