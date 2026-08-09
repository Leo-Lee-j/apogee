<script setup lang="ts">
import { computed } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { inTauri } from '@/ipc'
import { AppButton, AppIcon } from '@/components/ui'
import { IconColumns, IconRows, IconMaximize } from '@/components/ui/icons'
import MemoryMonitor from './MemoryMonitor.vue'
import { useAppStore } from '@/stores/app'

/** 状态栏，24px（02 文档 §3.2）。可在设置里隐藏。 */
const tabs = useTabsStore()
const ui = useUiStore()
const app = useAppStore()
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
    <MemoryMonitor v-if="app.settings.showMemoryMonitor" />
    <AppButton
      class="layout-toggle"
      quaternary
      circle
      size="small"
      :aria-label="`切换内容区布局，当前：${layoutLabel}`"
      :title="`${layoutLabel}，点击切换布局`"
      @click="ui.cycleLayout"
    >
      <template #icon>
        <AppIcon :component="ui.layout === 'stacked' ? IconRows : ui.layout === 'side-by-side' ? IconColumns : IconMaximize" :size="15" />
      </template>
    </AppButton>
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
  color: var(--a-text-faint);
}
.layout-toggle:hover { color: var(--a-text); }
</style>
