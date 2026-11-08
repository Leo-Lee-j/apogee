<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { inTauri } from '@/ipc'
import { AppButton, AppIcon } from '@/components/ui'
import { IconColumns, IconRows, IconMaximize } from '@/components/ui/icons'
import MemoryMonitor from './MemoryMonitor.vue'
import { useAppStore } from '@/stores/app'
import { useUpdateStore } from '@/stores/update'

/** 状态栏，24px（02 文档 §3.2）。可在设置里隐藏。 */
const tabs = useTabsStore()
const ui = useUiStore()
const app = useAppStore()
const updates = useUpdateStore()
onMounted(() => { void updates.loadVersion() })
function checkUpdate() {
  tabs.openSettings('about')
  if (!updates.busy && !updates.updateAvailable) void updates.check()
}
const dirtyCount = computed(() => tabs.tabs.filter((t) => t.dirty).length)
const layoutEnabled = computed(() => tabs.active?.kind === 'request' && !tabs.active.newPanel)
const layoutLabel = computed(() => ({
  stacked: '上下分栏',
  'side-by-side': '左右分栏',
  focus: '专注模式',
}[ui.layout]))
const proxyLabel = computed(() => {
  switch (app.settings.proxyMode) {
    case 'manual':
      return '手动'
    case 'none':
      return '关闭'
    default:
      return '系统'
  }
})
</script>

<template>
  <footer class="statusbar">
    <span class="ok">● 就绪</span>
    <span class="sep">|</span>
    <span>dev</span>
    <span class="sep">|</span>
    <span>代理: {{ proxyLabel }}</span>
    <span class="sep">|</span>
    <span v-if="dirtyCount">未保存 {{ dirtyCount }}</span>
    <span v-else class="dim">全部已保存</span>
    <span class="spacer" />
    <span class="dim">{{ inTauri ? 'Tauri' : '浏览器（Spike 对照）' }}</span>
    <span class="sep">|</span>
    <span class="dim">v{{ updates.currentVersion }}</span>
    <AppButton v-if="inTauri" quaternary size="tiny" title="打开关于页并检查 GitHub 发布标签" @click="checkUpdate">
      {{ updates.busy ? '更新处理中' : updates.updateAvailable ? `发现 v${updates.latest?.version}` : '检查更新' }}
    </AppButton>
    <MemoryMonitor v-if="app.settings.showMemoryMonitor" />
    <AppButton
      class="layout-toggle"
      quaternary
      circle
      size="small"
      :disabled="!layoutEnabled"
      :aria-label="`切换内容区布局，当前：${layoutLabel}`"
      :title="layoutEnabled ? `${layoutLabel}，点击切换布局` : '仅接口标签可用布局控制'"
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
  font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); color: var(--a-text-dim);
}
.ok { color: var(--a-ok); }
/* 分隔符是纯装饰，用 ghost 档；它不承载信息，按 WCAG 装饰元素豁免对比度。 */
.sep { color: var(--a-text-ghost); }
.dim { color: var(--a-text-faint); }
.spacer { flex: 1 1 auto; }
.layout-toggle {
  width: max(26px, var(--a-h-control-xs));
  height: var(--a-h-control-xs);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-left: 2px;
  padding: 0;
  color: var(--a-text-faint);
}
.layout-toggle:hover { color: var(--a-text); }
.layout-toggle:disabled { color: var(--a-text-ghost); cursor: not-allowed; opacity: 0.55; }
.layout-toggle:disabled:hover { background: transparent; color: var(--a-text-ghost); }
</style>
