<script setup lang="ts">
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'

/** 标签栏，34px。`•` 表示有未保存改动（02 文档 §3.2 / §4.5）。 */
const tabs = useTabsStore()
const ui = useUiStore()

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
    <!--
      `+` 打开新建面板，而不是直接开一个空标签。

      带一个 `▾`：这是在告诉用户「按下去会展开选择」。没有它，点 `+` 弹出面板
      会像误触 —— 用户的预期是「立刻多一个标签」。想要立刻多一个标签的人走
      `Ctrl+T`（02 文档 §6.2），那条路没有面板。
    -->
    <button
      class="add"
      type="button"
      aria-label="新建"
      aria-haspopup="dialog"
      :aria-expanded="ui.newOpen"
      title="新建（Ctrl+T 直接新建快捷请求）"
      @click="ui.newOpen = true"
    >
      +<span class="caret">▾</span>
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
  display: inline-flex; align-items: center; justify-content: center; gap: 1px;
  width: 38px; background: transparent; border: none; color: var(--a-text-dim);
  font-size: 16px; cursor: pointer;
}
.add:hover { background: var(--a-hover); color: var(--a-text); }
.add .caret { font-size: 9px; opacity: 0.7; }
</style>
