<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ipc, normalizeError, type TreeNode } from '@/ipc'

/**
 * JSON 树视图。
 *
 * # 关键约束（03 文档 §2.2 / §4.4）
 *
 * 前端拿不到完整 JSON 对象，只拿视口内那一页扁平节点。
 * 展开一个节点 = 向 Rust 再要一页 `children(path, offset, limit)`，
 * 而不是在前端递归遍历一棵已经在内存里的树。
 *
 * 数据结构是**扁平数组**：`TreeNode` 没有 `children` 字段。
 * 展开/折叠只改 visible 计算，不重建数组。
 */

const props = defineProps<{ responseId: string }>()

const PAGE = 200

/** 已加载的子节点，按父路径缓存 */
const children = ref<Record<string, TreeNode[]>>({})
const expanded = ref<Set<string>>(new Set(['']))
const loading = ref(true)
const err = ref<string | null>(null)

async function loadChildren(path: string) {
  if (children.value[path]) return
  const nodes = await ipc.getResponseTree(props.responseId, path, 0, PAGE)
  children.value = { ...children.value, [path]: nodes }
}

/** 把已展开的部分摊平成可渲染的一维列表。虚拟滚动就架在这个数组上。 */
const flat = computed(() => {
  const out: TreeNode[] = []
  const walk = (path: string) => {
    for (const n of children.value[path] ?? []) {
      out.push(n)
      if (expanded.value.has(n.path)) walk(n.path)
    }
  }
  walk('')
  return out
})

async function toggle(n: TreeNode) {
  if (n.childCount === 0) return
  const next = new Set(expanded.value)
  if (next.has(n.path)) {
    next.delete(n.path)
  } else {
    next.add(n.path)
    await loadChildren(n.path)
  }
  expanded.value = next
}

onMounted(async () => {
  try {
    await loadChildren('')
  } catch (e) {
    // ipc facade 抛的是 AppError（普通对象，不是 Error），
    // `String(e)` 会得到 "[object Object]"。非 JSON 响应走这条路很常见
    // （HTML/纯文本没建索引），错误文案必须能读。
    err.value = normalizeError(e).message
  } finally {
    loading.value = false
  }
})

const kindColor: Record<string, string> = {
  string: 'var(--a-get)',
  number: 'var(--a-patch)',
  bool: 'var(--a-put)',
  null: 'var(--a-text-faint)',
}
</script>

<template>
  <div class="tree">
    <div v-if="loading" class="state">解析中…</div>
    <div v-else-if="err" class="state err">
      树视图加载失败：{{ err }}
      <p class="dim">大响应体需要先显式点「解析」，这是 2MB 以上分级的设计行为。</p>
    </div>
    <div v-else class="rows">
      <div
        v-for="n in flat"
        :key="n.path"
        class="row"
        :style="{ paddingLeft: `${6 + (n.depth - 1) * 14}px` }"
        @click="toggle(n)"
      >
        <span class="caret" :class="{ has: n.childCount > 0 }">
          {{ n.childCount > 0 ? (expanded.has(n.path) ? '▾' : '▸') : '' }}
        </span>
        <span class="key">{{ n.key }}</span>
        <span class="colon">:</span>
        <span v-if="n.childCount > 0" class="count">
          {{ n.kind === 'array' ? `[ ${n.childCount} items ]` : `{ ${n.childCount} keys }` }}
        </span>
        <span v-else class="val" :style="{ color: kindColor[n.kind] ?? 'var(--a-text)' }">
          {{ n.preview }}
        </span>
      </div>
      <p class="note">
        已渲染 {{ flat.length }} 个节点。每次展开只向 Rust 取一页（{{ PAGE }} 个），
        前端不持有完整 JSON。
      </p>
    </div>
  </div>
</template>

<style scoped>
.tree { flex: 1 1 auto; min-height: 0; overflow: auto; }
.rows { font: 12px/20px var(--a-font-mono); }
.row { display: flex; gap: 4px; height: 20px; align-items: center; cursor: default; white-space: nowrap; }
.row:hover { background: var(--a-hover); }
.caret { width: 12px; color: var(--a-text-faint); }
.caret.has { cursor: pointer; }
.key { color: var(--a-text); }
.colon { color: var(--a-text-faint); }
.count { color: var(--a-text-faint); }
.val { overflow: hidden; text-overflow: ellipsis; }
.state { padding: 12px; font-size: 12px; color: var(--a-text-dim); }
.state.err { color: var(--a-error); }
.dim { color: var(--a-text-faint); }
.note { padding: 8px; font: 11px/16px var(--a-font-ui); color: var(--a-text-faint); }
</style>
