<script setup lang="ts">
import { useTabsStore } from '@/stores/tabs'

/** 标签栏，34px。`•` 表示有未保存改动（02 文档 §3.2 / §4.5）。 */
const tabs = useTabsStore()

const methodColor: Record<string, string> = {
  GET: 'var(--a-get)',
  POST: 'var(--a-post)',
  PUT: 'var(--a-put)',
  PATCH: 'var(--a-patch)',
  DELETE: 'var(--a-delete)',
}
</script>

<template>
  <div class="tabbar" role="tablist">
    <button
      v-for="t in tabs.tabs"
      :key="t.id"
      class="tab"
      :class="{ active: t.id === tabs.activeId, preview: t.preview }"
      role="tab"
      :aria-selected="t.id === tabs.activeId"
      :title="`${t.method} ${t.url || t.title}（${tabs.stateOf(t.id)}）`"
      @click="tabs.activate(t.id)"
    >
      <span class="m" :style="{ color: methodColor[t.method] ?? 'var(--a-head)' }">{{ t.method }}</span>
      <span class="t">{{ t.title }}</span>
      <span v-if="t.dirty" class="dot" aria-label="未保存">•</span>
      <span class="x" role="button" aria-label="关闭标签" @click.stop="tabs.close(t.id)">×</span>
    </button>
    <!-- 点击“+”创建独立的“新建请求”标签，选项面板只属于这个标签。 -->
    <button
      class="add"
      type="button"
      aria-label="新建请求"
      aria-controls="new-request-panel"
      :aria-expanded="tabs.active?.newPanel === true"
      title="新建请求"
      @click="tabs.openNewPanel()"
    >
      +
    </button>
  </div>
</template>

<style scoped>
.tabbar {
  height: var(--a-h-tabbar);
  flex: 0 0 auto;
  display: flex;
  align-items: stretch;
  background: var(--a-bg-2);
  border-bottom: 1px solid var(--a-border);
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}
.tabbar::-webkit-scrollbar { height: 0; }
.tab {
  display: flex; align-items: center; gap: 6px;
  padding: 0 8px 0 10px;
  max-width: 220px;
  background: transparent;
  border: none;
  border-right: 1px solid var(--a-border);
  color: var(--a-text-dim);
  font: inherit;
  cursor: pointer;
  white-space: nowrap;
}
.tab:hover { background: var(--a-hover); }
/* 激活态原来用 bg-1（画布色），是为了和下方内容区连成一片。改成块状分隔后
   下方是另一个块、中间隔着 gutter，这种「连接」不再存在，用 bg-1 反而会
   在块内部开一个洞。改用 bg-3（抬升面）表示选中。 */
.tab.active { background: var(--a-bg-3); color: var(--a-text); box-shadow: inset 0 -2px 0 var(--a-primary); }
.tab.preview .t { font-style: italic; }
.m { font: var(--a-fw-semibold) var(--a-fs-xs) / 1 var(--a-font-mono); }
.t { overflow: hidden; text-overflow: ellipsis; }
.dot { color: var(--a-primary); }
.x { width: 16px; text-align: center; border-radius: var(--a-radius-sm); opacity: 0.5; }
.x:hover { background: var(--a-hover); opacity: 1; }
.add {
  flex: 0 0 auto;
  display: inline-flex; align-items: center; justify-content: center;
  width: 38px; background: transparent; border: none; color: var(--a-text-dim);
  font-size: 16px; cursor: pointer;
}
.add:hover { background: var(--a-hover); color: var(--a-text); }
</style>
