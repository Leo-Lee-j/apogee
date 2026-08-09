<script setup lang="ts">
import { computed } from 'vue'
import { AppIcon } from '@/components/ui'
import { IconApi, IconFolder, IconFolderOpen, IconNetwork, IconStats } from '@/components/ui/icons'
import { useWorkspaceStore } from '@/stores/workspace'

const workspace = useWorkspaceStore()

const stats = computed(() => {
  const nodes = workspace.nodes ?? []
  const count = (kind: 'collection' | 'folder' | 'request') => nodes.filter((node) => node.kind === kind).length
  return {
    collections: count('collection'),
    folders: count('folder'),
    requests: count('request'),
    environments: workspace.environments.length,
    enabledVariables: workspace.environments.reduce(
      (total, environment) => total + environment.variables.filter((variable) => variable.enabled).length,
      0,
    ),
  }
})

const items = computed(() => [
  { label: '集合', value: stats.value.collections, icon: IconFolderOpen },
  { label: '文件夹', value: stats.value.folders, icon: IconFolder },
  { label: '请求', value: stats.value.requests, icon: IconApi },
  { label: '环境', value: stats.value.environments, icon: IconNetwork },
])
</script>

<template>
  <section class="workspace-stats a-block" aria-label="工作区统计">
    <header class="stats-head">
      <div>
        <h1>工作区统计</h1>
        <p>{{ workspace.workspaceName || '当前工作区' }}</p>
      </div>
      <AppIcon :component="IconStats" :size="20" aria-hidden="true" />
    </header>

    <div v-if="workspace.nodes === undefined" class="stats-empty">正在加载工作区数据。</div>
    <template v-else>
      <div class="stat-grid">
        <article v-for="item in items" :key="item.label" class="stat-item">
          <AppIcon :component="item.icon" :size="17" aria-hidden="true" />
          <span class="stat-value">{{ item.value }}</span>
          <span class="stat-label">{{ item.label }}</span>
        </article>
      </div>
      <div class="variables-row">
        <span>已启用环境变量</span>
        <strong>{{ stats.enabledVariables }}</strong>
      </div>
    </template>
  </section>
</template>

<style scoped>
.workspace-stats {
  flex: 1 1 auto;
  min-height: 0;
  padding: 24px;
  overflow: auto;
}
.stats-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; margin-bottom: 24px; }
.stats-head h1 { margin: 0; color: var(--a-text); font-size: 18px; font-weight: var(--a-fw-semibold); line-height: 28px; }
.stats-head p { margin: 2px 0 0; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.stats-head > :last-child { color: var(--a-primary); }
.stat-grid { display: grid; grid-template-columns: repeat(4, minmax(120px, 1fr)); gap: 12px; max-width: 760px; }
.stat-item { display: grid; grid-template-columns: auto 1fr; align-items: center; column-gap: 8px; min-height: 92px; padding: 16px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-2); }
.stat-item > :first-child { color: var(--a-text-faint); }
.stat-value { color: var(--a-text); font: var(--a-fw-semibold) 24px/1 var(--a-font-mono); }
.stat-label { grid-column: 1 / -1; margin-top: 12px; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.variables-row { display: flex; align-items: center; justify-content: space-between; max-width: 760px; margin-top: 16px; padding: 14px 16px; border-top: 1px solid var(--a-border); color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.variables-row strong { color: var(--a-text); font-family: var(--a-font-mono); font-size: var(--a-fs-lg); }
.stats-empty { display: grid; min-height: 160px; place-items: center; color: var(--a-text-faint); font-size: var(--a-fs-sm); }
@media (max-width: 760px) { .workspace-stats { padding: 16px; } .stat-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
