<script setup lang="ts">
import { ref, computed, shallowRef, watch, onMounted, onBeforeUnmount } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import {
  ROW_H,
  OVERSCAN,
  methodColor,
  synthTree,
  visibleNodes,
  type FlatNode,
} from './tree'

/**
 * 侧边栏。动态 import 挂载，不进首屏 chunk（03 文档 §4.1）。
 *
 * 集合树用自研虚拟滚动，不用 `n-tree`：这是三个热路径组件之一
 * （02 文档 §7.5 第 4 条）。1000 个请求首次展开要 < 100ms（PRD §5.1），
 * 通用组件库的树满足不了这个要求。
 *
 * 数据形状、虚拟滚动参数与 `visible` 过滤都在 `./tree.ts`，因为 **Spike S3 要量测
 * 的就是这个组件本身**（`src/spike/S3Panel.vue`），不能让量测台抄一份参数出来测副本。
 */

const props = defineProps<{
  /**
   * 外部注入的节点数组。不传则用内置合成数据。
   *
   * 正式实现里这里接 Rust 侧 `list_tree` 的返回；Spike S3 用它按 1000 / 5000 / 20000
   * 三档灌入压力数据。**不是**为了测试而留的后门 —— 数据源本来就该由外部提供。
   */
  nodes?: FlatNode[]
}>()

/**
 * 用 `shallowRef` 而不是 `ref`：这个数组在 S3 的 20000 节点档位下会产生
 * 20000 个 Proxy（03 文档 §4.2 明确禁止对大数组做深度响应式）。
 * 代价是改动节点内字段后必须手动换引用触发更新，见 `toggle`。
 */
const all = shallowRef<FlatNode[]>(props.nodes ?? synthTree({ folders: 40, perFolder: 25, expanded: 2 }))
watch(
  () => props.nodes,
  (next) => {
    if (next) all.value = next
  },
)

const filter = ref('')
const scrollTop = ref(0)
const viewportH = ref(400)
const scroller = ref<HTMLElement | null>(null)
const tabs = useTabsStore()

/** 可见节点：折叠的文件夹下的子节点不参与渲染。只改这个计算，不重建原数组。 */
const visible = computed(() => visibleNodes(all.value, filter.value))

const startIdx = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_H) - OVERSCAN))
const endIdx = computed(() =>
  Math.min(visible.value.length, Math.ceil((scrollTop.value + viewportH.value) / ROW_H) + OVERSCAN),
)
const slice = computed(() => visible.value.slice(startIdx.value, endIdx.value))
const totalH = computed(() => visible.value.length * ROW_H)

function onScroll(e: Event) {
  const el = e.target as HTMLElement
  scrollTop.value = el.scrollTop
  viewportH.value = el.clientHeight
}

/**
 * 视口高度必须在挂载时就从真实元素读出来，并跟着尺寸变化更新。
 *
 * **Spike S3 抓到的 bug**：原来 `viewportH` 初值写死 400，只在 `scroll` 事件里更新。
 * 于是首次渲染是按「400px 高的视口」算行数的 —— 侧边栏实际有 700px 时，
 * 底部会空出约 3 行，直到用户滚动一下才补上。窗口拉高、布局切换同样如此。
 * 虚拟滚动的可视区高度是输入参数，不是常量，不能猜。
 */
let resizeObserver: ResizeObserver | null = null
onMounted(() => {
  const el = scroller.value
  if (!el) return
  viewportH.value = el.clientHeight
  if (typeof ResizeObserver === 'undefined') return
  resizeObserver = new ResizeObserver(() => {
    viewportH.value = el.clientHeight
  })
  resizeObserver.observe(el)
})
onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})

function toggle(n: FlatNode) {
  if (!n.isFolder) {
    // 单击预览（02 文档 §4.5）：复用临时标签，避免浏览集合时开出一堆标签
    tabs.open({ title: n.name, method: n.method ?? 'GET', url: `{{baseUrl}}/${n.id}`, preview: true })
    return
  }
  n.expanded = !n.expanded
  // shallowRef 不追踪节点内部字段，换引用才能触发 visible 重算。
  // 数组本身不重建（slice 是浅拷贝），只是换个引用 —— 03 文档 §4.4 的
  // 「展开/折叠只改 visible 计算，不重建数组」指的正是这个。
  all.value = [...all.value]
}

/**
 * 供 Spike S3 驱动滚动与读取渲染状态。
 *
 * 只暴露量测需要的最小面：滚动容器（S3 要按帧改 `scrollTop`）以及
 * 三个规模数字（总数 / 可见 / 实渲染）。不暴露内部 ref，避免量测台反过来
 * 篡改组件状态。
 */
defineExpose({
  scroller,
  stats: () => ({
    total: all.value.length,
    visible: visible.value.length,
    rendered: slice.value.length,
    rowHeight: ROW_H,
    overscan: OVERSCAN,
    scrollHeight: totalH.value,
  }),
  /**
   * 程序化滚动。**同步**更新内部 `scrollTop`，不等 `scroll` 事件。
   *
   * 这不是给量测开的后门：「在树里定位到某个请求」「键盘上下键移动选中行」
   * 都需要这个入口（02 文档 §4.5 的单击预览与命令面板跳转都会用到）。
   * S3 顺便用它测「一次滚动步进的主线程成本」——走 `scroll` 事件是异步的，
   * 测不出单步开销，只能测到显示器节奏。
   */
  scrollTo: (px: number) => {
    scrollTop.value = px
    const el = scroller.value
    if (el) el.scrollTop = px
  },
  toggleFolderAt: (index: number) => {
    const node = all.value[index]
    if (node?.isFolder) toggle(node)
  },
})
</script>

<template>
  <aside class="sidebar">
    <div class="head">
      <input v-model="filter" class="filter" placeholder="过滤请求…" aria-label="过滤请求" spellcheck="false" />
    </div>
    <div ref="scroller" class="scroller" @scroll.passive="onScroll">
      <div class="spacer" :style="{ height: `${totalH}px` }">
        <div
          v-for="(n, i) in slice"
          :key="n.id"
          class="row"
          :class="{ folder: n.isFolder }"
          :style="{
            transform: `translateY(${(startIdx + i) * ROW_H}px)`,
            paddingLeft: `${6 + n.depth * 12}px`,
          }"
          @click="toggle(n)"
        >
          <span v-if="n.isFolder" class="caret">{{ n.expanded ? '▾' : '▸' }}</span>
          <span v-else class="m" :style="{ color: methodColor[n.method!] ?? 'var(--a-head)' }">{{ n.method }}</span>
          <span class="name">{{ n.name }}</span>
        </div>
      </div>
    </div>
    <div class="foot">
      共 {{ all.length }} 项，可见 {{ visible.length }}，实渲染 {{ slice.length }}
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  width: var(--a-w-sidebar); flex: 0 0 auto;
  display: flex; flex-direction: column; min-height: 0;
  /* 没有 border-right —— 与主区之间靠 --a-gutter 的间隙分隔（02 文档 §3.2）。
     背景由 .a-block 提供，这里保留一份是为了组件单独使用时也有底色。 */
  background: var(--a-bg-2);
}
.head { padding: 6px; flex: 0 0 auto; }
.filter {
  width: 100%; height: 24px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: 4px;
  color: var(--a-text); font: 12px/1 var(--a-font-ui); padding: 0 6px;
}
/* 不能写 outline: none —— 那会盖掉 tokens.css 里的全局 :focus-visible 焦点环
   （`.filter:focus` 特异性高于 `:focus-visible`），等于把键盘焦点指示器去掉，
   违反 PRD §10.4。改成把焦点环画在边框内侧。 */
.filter:focus-visible {
  border-color: var(--a-focus);
  outline: 2px solid var(--a-focus);
  outline-offset: -2px;
}
.scroller { flex: 1 1 auto; overflow-y: auto; min-height: 0; }
.spacer { position: relative; }
.row {
  position: absolute; left: 0; right: 0; height: 22px;
  display: flex; align-items: center; gap: 5px;
  font: 12px/1 var(--a-font-ui); color: var(--a-text-dim);
  white-space: nowrap; cursor: pointer;
}
.row:hover { background: var(--a-hover); color: var(--a-text); }
.row.folder { color: var(--a-text); }
.caret { width: 10px; color: var(--a-text-faint); }
.m { font: 600 10px/1 var(--a-font-mono); width: 42px; }
.name { overflow: hidden; text-overflow: ellipsis; }
.foot {
  flex: 0 0 auto; padding: 3px 6px; border-top: 1px solid var(--a-border);
  font: 10px/14px var(--a-font-mono); color: var(--a-text-faint);
}
</style>
