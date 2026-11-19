<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { createJsonTree, TREE_PAGE_SIZE } from '@/features/jsonTree'
import type { TreeNode } from '@/ipc'

const props = defineProps<{ responseId: string }>()
const tree = createJsonTree()
const { rows, expanded, rootPath } = tree
const viewport = ref<HTMLElement | null>(null)
const scrollTop = ref(0), height = ref(400), message = ref('')
// Fixed rows keep virtual scroll stable through loading and error states.
const ROW_HEIGHT = 32, OVERSCAN = 5
const start = computed(() => Math.max(0, Math.min(rows.value.length - 1, Math.floor(scrollTop.value / ROW_HEIGHT)) - OVERSCAN))
const visible = computed(() => rows.value.slice(start.value, start.value + Math.ceil(height.value / ROW_HEIGHT) + OVERSCAN * 2))
let observer: ResizeObserver | undefined
watch(() => props.responseId, id => {
  message.value = ''; scrollTop.value = 0
  if (viewport.value) viewport.value.scrollTop = 0
  void tree.reset(id)
}, { immediate: true })
onMounted(() => {
  observer = new ResizeObserver(entries => { height.value = entries[0]?.contentRect.height ?? 400 })
  if (viewport.value) observer.observe(viewport.value)
})
onBeforeUnmount(() => { observer?.disconnect(); tree.dispose() })
async function toggle(node: TreeNode) { message.value = await tree.toggle(node) ?? '' }
function focus(path: string) {
  message.value = ''; scrollTop.value = 0
  if (viewport.value) viewport.value.scrollTop = 0
  void tree.focus(path)
}
function jump(path: string, event: Event) {
  const page = Number((event.target as HTMLInputElement).value)
  const total = Math.ceil((tree.pages.value[path]?.total ?? 0) / TREE_PAGE_SIZE)
  if (Number.isInteger(page) && page >= 1 && page <= total) void tree.load(path, (page - 1) * TREE_PAGE_SIZE)
}
const kindColor: Record<string, string> = { string: 'var(--a-get)', number: 'var(--a-patch)', bool: 'var(--a-put)', null: 'var(--a-text-faint)' }
</script>

<template>
  <div class="tree-view">
    <div v-if="rootPath" class="tree-navigation"><button @click="focus('')">返回根节点</button><button @click="focus(rootPath.slice(0, rootPath.lastIndexOf('/')))">返回上级</button><span :title="rootPath">{{ rootPath }}</span></div>
    <div v-if="message" class="message" role="status">{{ message }}</div>
    <div ref="viewport" class="tree" aria-label="JSON 树" @scroll="scrollTop = ($event.target as HTMLElement).scrollTop">
      <div class="rows" :style="{ height: `${rows.length * ROW_HEIGHT}px` }">
        <div class="window" :style="{ transform: `translateY(${start * ROW_HEIGHT}px)` }">
          <div v-for="row in visible" :key="row.key" class="row" :style="{ paddingLeft: `${6 + row.depth * 14}px` }">
            <template v-if="row.type === 'page'">
              <span class="range">{{ row.page.total ? `${row.page.offset + 1}–${row.page.offset + row.page.nodes.length} / ${row.page.total} 项` : (row.page.loading ? '加载中…' : '无子项') }}</span>
              <button v-if="row.page.total > TREE_PAGE_SIZE" :disabled="row.page.loading || row.page.offset === 0" @click="tree.load(row.path, row.page.offset - TREE_PAGE_SIZE)">上一页</button>
              <button v-if="row.page.total > TREE_PAGE_SIZE" :disabled="row.page.loading || row.page.offset + TREE_PAGE_SIZE >= row.page.total" @click="tree.load(row.path, row.page.offset + TREE_PAGE_SIZE)">下一页</button>
              <label v-if="row.page.total > TREE_PAGE_SIZE">页 <input type="number" aria-label="跳转页码" :min="1" :max="Math.ceil(row.page.total / TREE_PAGE_SIZE)" :value="row.page.offset / TREE_PAGE_SIZE + 1" :disabled="row.page.loading" @change="jump(row.path, $event)" @keydown.enter="jump(row.path, $event)" /></label>
              <span v-if="row.page.error" class="error" role="alert" :title="row.page.error">{{ row.page.error }}</span>
              <button v-if="row.page.error" @click="tree.load(row.path, row.page.retryOffset ?? row.page.offset)">重试</button>
              <span v-else-if="row.page.loading && row.page.nodes.length" class="range">加载中…</span>
            </template>
            <template v-else>
              <button v-if="row.node.childCount > 0" class="caret" :aria-label="`${expanded.has(row.node.path) ? '折叠' : '展开'} ${row.node.key}`" :aria-expanded="expanded.has(row.node.path)" @click="toggle(row.node)">{{ expanded.has(row.node.path) ? '▾' : '▸' }}</button>
              <span v-else class="caret"></span>
              <span class="key">{{ row.node.key }}</span><span class="colon">:</span>
              <span v-if="row.node.childCount > 0" class="range">{{ row.node.kind === 'array' ? `[ ${row.node.childCount} items ]` : `{ ${row.node.childCount} keys }` }}</span>
              <span v-else class="val" :style="{ color: kindColor[row.node.kind] ?? 'var(--a-text)' }">{{ row.node.preview }}</span>
              <button v-if="row.node.childCount > 0" class="focus-node" :aria-label="`仅查看 ${row.node.key}`" @click="focus(row.node.path)">仅查看</button>
            </template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.tree-view { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; }
.tree { flex: 1 1 auto; min-height: 0; overflow: auto; overflow-anchor: none; }
.rows { position: relative; font: var(--a-fs-result)/var(--a-lh-result) var(--a-font-mono); }
.window { position: absolute; left: 0; right: 0; top: 0; }
.row { display: flex; gap: 4px; height: 32px; box-sizing: border-box; align-items: center; white-space: nowrap; }
.row:hover { background: var(--a-hover); }
button, input { color: var(--a-text); background: var(--a-bg-2); border: 1px solid var(--a-border); border-radius: 3px; font: inherit; }
button { cursor: pointer; padding: 1px 5px; }
button:disabled { opacity: 0.5; cursor: default; }
button:focus-visible, input:focus-visible { outline: 2px solid var(--a-focus); outline-offset: 1px; }
input { width: 58px; }
.caret { width: 18px; flex-shrink: 0; color: var(--a-text-dim); padding: 0; background: transparent; border: none; }
.key { color: var(--a-text); }
.colon, .range { color: var(--a-text-faint); }
.val, .error { overflow: hidden; text-overflow: ellipsis; }
.error { color: var(--a-error); max-width: 360px; }
.message { padding: 6px; color: var(--a-text-dim); }
.tree-navigation { display: flex; gap: 8px; align-items: center; padding: 4px 6px; color: var(--a-text-dim); }
.tree-navigation span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.focus-node { margin-left: 6px; font-size: var(--a-fs-xs); }
</style>
