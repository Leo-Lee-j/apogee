<script setup lang="ts">
import { computed, ref } from 'vue'
import { ROW_H, methodColor, visibleNodes, type FlatNode } from '@/components/sidebar/tree'

/**
 * Spike S3 的**对照组**：不做虚拟滚动，`v-for` 把所有可见行一次性铺进 DOM。
 *
 * 这不是产品代码，也不打算变成产品代码。它存在的唯一目的是把「自研虚拟滚动」
 * 的收益量化成数字 —— 和 S2 里「把整个 body 拉进 JS 拼字符串」那个对照组同一性质：
 * 它是竞品（以及任何直接用组件库树控件的实现）的实际行为。
 *
 * 除了不虚拟化，其它一切都与 `Sidebar.vue` 保持一致：同样的行高、同样的
 * `visibleNodes` 过滤、同样的行内标记与样式。否则测出来的差值里会混进无关变量。
 */

const props = defineProps<{ nodes: FlatNode[] }>()

const scroller = ref<HTMLElement | null>(null)
const visible = computed(() => visibleNodes(props.nodes))

defineExpose({
  scroller,
  stats: () => ({
    total: props.nodes.length,
    visible: visible.value.length,
    // 全量渲染下「实渲染」恒等于「可见」—— 这正是它的代价所在
    rendered: visible.value.length,
    rowHeight: ROW_H,
    overscan: 0,
    scrollHeight: visible.value.length * ROW_H,
  }),
  /**
   * 与 Sidebar 同名同义，供 S3 测单步滚动成本。
   * 对照组没有内部 `scrollTop` 状态可更新 —— 这正是全量渲染的「优点」：
   * 滚动完全由合成器处理，主线程不参与。单步成本因此接近 0，
   * 代价全部前置到首渲的 O(N) 布局上（见报告的 commit/patch 列）。
   */
  scrollTo: (px: number) => {
    const el = scroller.value
    if (el) el.scrollTop = px
  },
  toggleFolderAt: () => {
    /* 对照组不测展开/折叠，只测首渲与滚动 */
  },
})
</script>

<template>
  <aside class="full-tree">
    <div ref="scroller" class="scroller">
      <div
        v-for="n in visible"
        :key="n.id"
        class="row"
        :class="{ folder: n.isFolder }"
        :style="{ paddingLeft: `${6 + n.depth * 12}px` }"
      >
        <span v-if="n.isFolder" class="caret">{{ n.expanded ? '▾' : '▸' }}</span>
        <span v-else class="m" :style="{ color: methodColor[n.method!] ?? 'var(--a-head)' }">{{ n.method }}</span>
        <span class="name">{{ n.name }}</span>
      </div>
    </div>
  </aside>
</template>

<style scoped>
.full-tree {
  width: var(--a-w-sidebar); flex: 0 0 auto;
  display: flex; flex-direction: column; min-height: 0;
  background: var(--a-bg-2);
}
.scroller { flex: 1 1 auto; overflow-y: auto; min-height: 0; }
/* 与 Sidebar 的行样式一致，只是靠文档流堆叠而不是 absolute + translateY */
.row {
  height: 22px; display: flex; align-items: center; gap: 5px;
  font: 12px/1 var(--a-font-ui); color: var(--a-text-dim);
  white-space: nowrap; cursor: pointer;
}
.row.folder { color: var(--a-text); }
.caret { width: 10px; color: var(--a-text-faint); }
.m { font: 600 10px/1 var(--a-font-mono); width: 42px; }
.name { overflow: hidden; text-overflow: ellipsis; }
</style>
