<script setup lang="ts">
import { useTabsStore } from '@/stores/tabs'
import EnvironmentControls from '@/components/layout/EnvironmentControls.vue'
import { AppIcon } from '@/components/ui'
import { IconAdd, IconClose, IconDocument, IconFlash } from '@/components/ui/icons'

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
  <div class="tabbar">
    <div class="tab-strip" role="tablist" aria-label="工作区标签">
      <button
        v-for="t in tabs.tabs"
        :key="t.id"
        class="tab"
        :class="{ active: t.id === tabs.activeId, preview: t.preview }"
        role="tab"
        :aria-selected="t.id === tabs.activeId"
        :title="t.kind === 'request' && !t.newPanel ? `${t.method} ${t.url || t.title}（${tabs.stateOf(t.id)}）` : t.title"
        @click="tabs.activate(t.id)"
      >
        <!-- 新建请求面板：还没选请求类型，不冒充 GET，用闪电图标表示「待选择」 -->
        <span v-if="t.newPanel && t.kind === 'request'" class="m new-bolt" aria-hidden="true">
          <AppIcon :component="IconFlash" :size="13" />
        </span>
        <span
          v-else-if="t.kind === 'request'"
          class="m"
          :style="{ color: methodColor[t.method] ?? 'var(--a-head)' }"
        >{{ t.method }}</span>
        <span v-else class="page-icon" aria-hidden="true"><AppIcon :component="IconDocument" :size="13" /></span>
        <span class="t">{{ t.title }}</span>
        <span v-if="t.dirty" class="dot" aria-label="未保存">•</span>
        <span class="x" role="button" aria-label="关闭标签" @click.stop="tabs.close(t.id)"><AppIcon :component="IconClose" :size="13" /></span>
      </button>
      <button
        class="add"
        type="button"
        aria-label="新建请求"
        aria-controls="new-request-panel"
        :aria-expanded="tabs.active?.newPanel === true"
        title="新建请求"
        @click="tabs.openNewPanel()"
      >
        <AppIcon :component="IconAdd" :size="16" />
      </button>
    </div>
    <EnvironmentControls />
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
  overflow: visible;
}
.tab-strip {
  flex: 1 1 auto;
  min-width: 0;
  display: flex;
  align-items: stretch;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}
.tab-strip::-webkit-scrollbar { height: 0; }
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
/* 新建请求面板的占位图标：与内容区「快捷请求」卡片的闪电同色（--a-primary），
   让标签图标和内容区的图标一眼对应；普通标签仍用方法彩色文本。 */
.new-bolt { display: inline-flex; align-items: center; justify-content: center; width: 14px; color: var(--a-primary); }
.page-icon { width: 14px; color: var(--a-text-faint); line-height: 1; }
.t { overflow: hidden; text-overflow: ellipsis; }
.dot { color: var(--a-primary); }
.x { width: 16px; height: 18px; display: inline-flex; align-items: center; justify-content: center; border-radius: var(--a-radius-sm); opacity: 0.5; }
.x:hover { background: var(--a-hover); opacity: 1; }
.add {
  flex: 0 0 auto;
  display: inline-flex; align-items: center; justify-content: center;
  width: 38px; background: transparent; border: none; color: var(--a-text-dim);
  font-size: 16px; cursor: pointer;
}
.add:hover { background: var(--a-hover); color: var(--a-text); }
</style>
