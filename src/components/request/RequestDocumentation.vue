<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { RequestDocViewModel } from '@/features/requestDocumentation'

const props = withDefaults(defineProps<{
  documentation: RequestDocViewModel
  pendingBulkEdit?: boolean
}>(), {
  pendingBulkEdit: false,
})

const methodClass = computed(() => `method-${props.documentation.method.toLowerCase()}`)
const copyState = ref('')
let copyTimer: number | undefined

function showCopyState(message: string) {
  copyState.value = message
  if (copyTimer !== undefined) window.clearTimeout(copyTimer)
  copyTimer = window.setTimeout(() => {
    copyState.value = ''
    copyTimer = undefined
  }, 1800)
}

async function copyUrl() {
  if (!props.documentation.copyUrl) return
  try {
    await navigator.clipboard.writeText(props.documentation.copyUrl)
    showCopyState('接口地址已复制')
  } catch (error) {
    showCopyState(error instanceof Error ? error.message : '复制失败')
  }
}

watch(
  () => [props.documentation.title, props.documentation.url],
  () => {
    copyState.value = ''
  },
)

onBeforeUnmount(() => {
  if (copyTimer !== undefined) window.clearTimeout(copyTimer)
})
</script>

<template>
  <article class="request-documentation" aria-label="接口文档（只读）">
    <header class="doc-overview">
      <div class="doc-title-row">
        <span class="a-pill doc-method" :class="methodClass">{{ documentation.method }}</span>
        <div class="doc-title-wrap">
          <h2>{{ documentation.title }}</h2>
          <div class="doc-state" aria-label="文档状态">
            <span>接口说明 · 只读</span>
            <span v-if="props.pendingBulkEdit" class="doc-pending">批量编辑草稿未应用</span>
            <span v-if="documentation.dirty" class="doc-dirty">有未保存修改</span>
          </div>
        </div>
      </div>
      <div class="doc-url-row">
        <code class="doc-url" :title="documentation.url">{{ documentation.url }}</code>
        <button
          type="button"
          class="doc-copy"
          :disabled="!documentation.copyUrl"
          title="复制接口地址模板"
          @click="copyUrl"
        >
          复制地址
        </button>
      </div>
    </header>

    <p v-if="copyState" class="doc-copy-state" role="status">{{ copyState }}</p>
    <p class="doc-hint">用于开发对接：只展示接口结构、参数类型和说明，不展示请求示例值。</p>

    <p v-if="documentation.isEmpty" class="doc-empty-state">当前请求尚未配置参数、请求体或鉴权。</p>

    <template v-else>
      <section v-for="section in documentation.sections" :key="section.id" class="doc-section">
        <header class="doc-section-head">
          <h3>{{ section.title }}</h3>
          <span v-if="section.count != null" class="doc-count">{{ section.count }} 项</span>
        </header>

        <div v-if="section.kind === 'table'" class="doc-table" role="table" :aria-label="section.title">
          <div class="doc-row doc-row-head" role="row">
            <span>参数名</span>
            <span>类型</span>
            <span>说明</span>
            <span>状态</span>
          </div>
          <template v-if="section.rows?.length">
            <div
              v-for="(row, rowIndex) in section.rows"
              :key="`${section.id}-${rowIndex}-${row.name}`"
              class="doc-row"
              :class="{ 'doc-row-off': row.status === '未启用' }"
              role="row"
            >
              <code>{{ row.name }}</code>
              <span class="doc-type">{{ row.type }}</span>
              <span class="doc-description">{{ row.description }}</span>
              <span class="doc-status">{{ row.status }}</span>
            </div>
          </template>
          <p v-else class="doc-empty">暂无参数定义。</p>
        </div>

        <div v-else-if="section.kind === 'tree'" class="doc-tree" role="tree" :aria-label="section.title">
          <div class="doc-tree-row doc-tree-head" role="row">
            <span>参数</span>
            <span>类型</span>
            <span>说明</span>
            <span>状态</span>
          </div>
          <template v-if="section.tree?.length">
            <div
              v-for="(row, rowIndex) in section.tree"
              :key="`${section.id}-${rowIndex}-${row.depth}-${row.name}`"
              class="doc-tree-row"
              :class="{ 'doc-row-off': row.status === '未启用' }"
              role="treeitem"
              :aria-level="row.depth + 1"
            >
              <span class="doc-tree-name" :style="{ paddingLeft: `${row.depth * 16 + 9}px` }">
                <span class="doc-tree-marker" aria-hidden="true">{{ row.hasChildren ? '▾' : '·' }}</span>
                <code>{{ row.name }}</code>
              </span>
              <span class="doc-type">{{ row.type }}</span>
              <span class="doc-description">{{ row.description }}</span>
              <span class="doc-status">{{ row.status }}</span>
            </div>
          </template>
          <p v-else class="doc-empty">{{ section.detail || '暂无参数定义。' }}</p>
        </div>

        <div v-else class="doc-summary">
          <strong>{{ section.value }}</strong>
          <span v-if="section.detail">{{ section.detail }}</span>
        </div>
      </section>
    </template>
  </article>
</template>

<style scoped>
.request-documentation {
  min-height: 100%;
  box-sizing: border-box;
  padding: 12px 14px 20px;
  color: var(--a-text-soft);
  font: var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui);
}
.doc-overview { padding: 2px 0 14px; border-bottom: 1px solid var(--a-border); }
.doc-title-row { display: flex; align-items: flex-start; gap: 9px; min-width: 0; }
.doc-method {
  flex: 0 0 auto;
  margin-top: 1px;
  font-family: var(--a-font-mono);
}
.doc-method.method-get { color: var(--a-get); background: var(--a-get-bg); }
.doc-method.method-post { color: var(--a-post); background: var(--a-post-bg); }
.doc-method.method-put { color: var(--a-put); background: var(--a-put-bg); }
.doc-method.method-patch { color: var(--a-patch); background: var(--a-patch-bg); }
.doc-method.method-delete { color: var(--a-delete); background: var(--a-delete-bg); }
.doc-method.method-head, .doc-method.method-options { color: var(--a-head); background: var(--a-head-bg); }
.doc-method:not(.method-get):not(.method-post):not(.method-put):not(.method-patch):not(.method-delete):not(.method-head):not(.method-options) {
  color: var(--a-text-soft);
  background: var(--a-bg-3);
}
.doc-title-wrap { min-width: 0; }
.doc-title-wrap h2 {
  margin: 0;
  overflow: hidden;
  color: var(--a-text);
  font-size: var(--a-fs-md);
  font-weight: 600;
  line-height: var(--a-lh-md);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.doc-state { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 3px; color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.doc-pending, .doc-dirty { color: var(--a-warn); }
.doc-url-row { display: flex; align-items: flex-start; gap: 8px; min-width: 0; margin-top: 10px; }
.doc-url {
  min-width: 0;
  flex: 1 1 auto;
  overflow-wrap: anywhere;
  color: var(--a-text-soft);
  font: var(--a-fs-editor-sm)/var(--a-lh-editor) var(--a-font-editor);
  user-select: text;
}
.doc-copy {
  flex: 0 0 auto;
  min-height: var(--a-h-control-xs);
  padding: 0 8px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text-dim);
  font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-ui);
  cursor: pointer;
}
.doc-copy:hover:not(:disabled) { border-color: var(--a-primary); color: var(--a-primary); background: var(--a-hover); }
.doc-copy:disabled { cursor: not-allowed; opacity: .45; }
.doc-copy-state { margin: 8px 0 -2px; color: var(--a-primary); font-size: var(--a-fs-xs); }
.doc-hint { margin: 10px 0 0; color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.doc-empty-state { margin: 16px 0 0; padding: 12px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-2); color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.doc-section { margin-top: 16px; }
.doc-section-head { display: flex; align-items: baseline; gap: 8px; min-height: 28px; }
.doc-section-head h3 { margin: 0; color: var(--a-text); font-size: var(--a-fs-sm); font-weight: 600; line-height: var(--a-lh-sm); }
.doc-count { color: var(--a-text-faint); font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); }
.doc-table, .doc-tree { overflow-x: auto; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-2); }
.doc-row, .doc-tree-row {
  min-width: 560px;
  min-height: var(--a-h-result-row);
  display: grid;
  grid-template-columns: minmax(140px, 24%) minmax(100px, 20%) minmax(180px, 1fr) 72px;
  border-bottom: 1px solid var(--a-border);
}
.doc-row:last-child, .doc-tree-row:last-child { border-bottom: 0; }
.doc-row > *, .doc-tree-row > * { min-width: 0; padding: 6px 9px; overflow-wrap: anywhere; border-right: 1px solid var(--a-border); }
.doc-row > :last-child, .doc-tree-row > :last-child { border-right: 0; }
.doc-row code, .doc-tree-row code { color: var(--a-post); font: var(--a-fs-result-sm)/var(--a-lh-result-sm) var(--a-font-mono); }
.doc-row-head, .doc-tree-head { min-height: var(--a-h-control-xs); background: var(--a-bg-3); color: var(--a-text-faint); font-size: var(--a-fs-xs); font-weight: var(--a-fw-medium); }
.doc-row-head > *, .doc-tree-head > * { padding-top: 5px; padding-bottom: 5px; }
.doc-row-off { opacity: .48; }
.doc-type { color: var(--a-text-soft); font-family: var(--a-font-mono); }
.doc-description { color: var(--a-text-dim); }
.doc-status { color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.doc-row:not(.doc-row-off) .doc-status, .doc-tree-row:not(.doc-row-off) .doc-status { color: var(--a-ok); }
.doc-tree-name { display: flex; align-items: baseline; gap: 5px; min-width: 0; }
.doc-tree-marker { flex: 0 0 10px; color: var(--a-text-faint); font-family: var(--a-font-mono); }
.doc-empty { margin: 0; min-width: 560px; padding: 10px 9px; color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.doc-summary { display: flex; flex-wrap: wrap; align-items: baseline; gap: 8px; min-height: var(--a-h-result-row); padding: 8px 10px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-2); }
.doc-summary strong { color: var(--a-text-soft); font-weight: var(--a-fw-medium); }
.doc-summary span { color: var(--a-text-faint); font-size: var(--a-fs-xs); }
@media (max-width: 620px) {
  .request-documentation { padding-right: 10px; padding-left: 10px; }
  .doc-url-row { align-items: stretch; flex-direction: column; }
  .doc-copy { align-self: flex-start; }
}
</style>
