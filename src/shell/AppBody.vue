<script setup lang="ts">
import { defineAsyncComponent, computed, nextTick, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import TitleBar from '@/components/layout/TitleBar.vue'
import WorkspaceArea from '@/components/layout/WorkspaceArea.vue'
import StatusBar from '@/components/layout/StatusBar.vue'
import UrlBar from '@/components/request/UrlBar.vue'
import RequestPanel from '@/components/request/RequestPanel.vue'
import ResponsePanel from '@/components/response/ResponsePanel.vue'
import { useUiStore, SIDEBAR_MIN_W, SPLIT_DEFAULT_RATIO, SPLIT_MAX_RATIO, SPLIT_MIN_RATIO } from '@/stores/ui'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import { ipc } from '@/ipc'

/**
 * 布局主体。两个 shell 变体共用这一份，差别只在外层有没有 naive-ui 的
 * ConfigProvider（见 Spike S5）。
 *
 * # 首屏预算（02 文档 §1）
 *
 * 首屏静态导入的只有：顶栏、标签栏、URL 栏、请求区、响应区壳、状态栏。
 * 下面这些全部动态 import，不进首屏 chunk：
 *   - 侧边栏（含虚拟树）
 *   - 设置页（naive-ui 表单组件）
 *   - 命令面板
 *   - CodeMirror（在 ResponsePanel 内部再下一层动态 import）
 *   - Spike 面板
 */

const Sidebar = defineAsyncComponent(() => import('@/components/sidebar/Sidebar.vue'))
const SettingsPanel = defineAsyncComponent(() => import('@/views/SettingsModal.vue'))
const HistoryPanel = defineAsyncComponent(() => import('@/views/HistoryPanel.vue'))
const CommandPalette = defineAsyncComponent(() => import('@/views/CommandPalette.vue'))
const NewPanel = defineAsyncComponent(() => import('@/views/NewPanel.vue'))
const S2Panel = defineAsyncComponent(() => import('@/spike/S2Panel.vue'))
const S3Panel = defineAsyncComponent(() => import('@/spike/S3Panel.vue'))

const ui = useUiStore()
const tabs = useTabsStore()
const workspace = useWorkspaceStore()

/** 侧边栏延后到首帧之后再挂，避免和首屏抢主线程（03 文档 §4.1 分阶段启动）。 */
const sidebarReady = ref(false)
onMounted(() => {
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      sidebarReady.value = true
    }),
  )
  // S2 用构建期开关（历史原因，保留）
  if (import.meta.env.VITE_AUTORUN_S2 === '1') {
    ui.spikePanel = 's2'
    ui.spikeOpen = true
  }
  // S3 用运行时开关：同一个 release 二进制要跑 GPU 开/关两种配置，
  // 构建期开关会导致每换一次配置就重链一次（见 ipc.spikeAutorun 的注释）。
  void ipc
    .spikeAutorun()
    .then((which) => {
      if (!which) return
      // `s3-mem` 是 S3 的「只测内存」模式（干净启动直接进驻留），面板同一个。
      // 这里只负责把面板挂起来，具体跑哪种模式由面板自己再读一次 autorun 值决定。
      const panel = which.startsWith('s3') ? 's3' : which === 's2' ? 's2' : null
      if (!panel) return
      ui.spikePanel = panel
      ui.spikeOpen = true
    })
    .catch(() => {
      /* 非 Tauri 环境没有这个命令，正常运行不受影响 */
    })
})

// ── 侧边栏拖拽 ────────────────────────────────────────────────────────

/**
 * 宽度写在 AppBody 而不是 Sidebar 自己身上。
 *
 * Sidebar.vue 的 `width: var(--a-w-sidebar)` 留作默认值，这里用行内样式覆盖。
 * 反过来做（让 Sidebar 直接读 store）会污染 Spike S3：量测台把同一个组件挂在
 * 一个固定 240px 的量测位里，视口宽度直接决定要渲染多少行，不能让它跟着
 * 用户拖出来的宽度漂。
 */
const sidebarStyle = computed(() => ({ width: `${ui.sidebarWidth}px` }))
const dragging = ref(false)

/**
 * 用 Pointer Events + setPointerCapture，不用 document 上的 mousemove。
 *
 * 捕获之后指针事件全部改派到分隔条本身，于是：拖过 CodeMirror 或 iframe 时
 * 不丢事件、指针移出窗口再回来也不用补状态、松手一定收得到 pointerup。
 * 这是「在别人的内容上方拖动」这类交互唯一稳的做法。
 */
function onResizeStart(e: PointerEvent) {
  if (e.button !== 0) return
  const bar = e.currentTarget as HTMLElement
  const startX = e.clientX
  const startW = ui.sidebarWidth
  bar.setPointerCapture(e.pointerId)
  dragging.value = true

  const onMove = (ev: PointerEvent) => ui.setSidebarWidth(startW + ev.clientX - startX)
  const onEnd = () => {
    dragging.value = false
    bar.removeEventListener('pointermove', onMove)
    bar.removeEventListener('pointerup', onEnd)
    bar.removeEventListener('pointercancel', onEnd)
  }
  bar.addEventListener('pointermove', onMove)
  bar.addEventListener('pointerup', onEnd)
  bar.addEventListener('pointercancel', onEnd)
  // 阻止把分隔条当文本拖选
  e.preventDefault()
}

/** 键盘也能调宽度：分隔条是可聚焦的 separator，方向键 8px 一档（PRD §10.4）。 */
function onResizeKey(e: KeyboardEvent) {
  const step = e.shiftKey ? 40 : 8
  if (e.key === 'ArrowLeft') ui.setSidebarWidth(ui.sidebarWidth - step)
  else if (e.key === 'ArrowRight') ui.setSidebarWidth(ui.sidebarWidth + step)
  else if (e.key === 'Home') ui.setSidebarWidth(SIDEBAR_MIN_W)
  else if (e.key === 'End') ui.setSidebarWidth(ui.sidebarMaxW)
  else if (e.key === 'Enter' || e.key === ' ') ui.resetSidebarWidth()
  else return
  e.preventDefault()
}

/**
 * 窗口变窄时把宽度回夹一次。
 *
 * 不做的话：在大窗口里拖到 480，再把窗口缩到 900，侧边栏会继续占 480 而主区
 * 只剩 400 —— URL 栏开始挤压，而用户并没有做过任何拖拽操作。
 */
function onWindowResize() {
  ui.syncViewport(window.innerWidth)
}
let splitObserver: ResizeObserver | null = null
onMounted(() => {
  onWindowResize()
  window.addEventListener('resize', onWindowResize)
  if (typeof ResizeObserver !== 'undefined') {
    splitObserver = new ResizeObserver(readSplitMetrics)
    observeSplit()
  }
})
onBeforeUnmount(() => {
  window.removeEventListener('resize', onWindowResize)
  splitObserver?.disconnect()
})

const splitStyle = computed(() =>
  ui.layout === 'side-by-side'
    ? { flexDirection: 'row' as const }
    : { flexDirection: 'column' as const },
)
const splitEl = ref<HTMLElement | null>(null)
const splitSize = ref(0)
const splitGutter = ref(3)
const splitDragging = ref(false)

const activeNewPanel = computed(() => tabs.active?.newPanel === true)
const activeKind = computed(() => tabs.active?.kind ?? 'request')

/**
 * 每个区域至少保留当前 splitter 容器方向尺寸的 1/3。
 *
 * Flex 比例计算会扣除中间 3px 间隙，所以把「容器尺寸 / 3」换算成可分配空间
 * 的比例，900×600 时实际边界约为 300×200，而不是把间隙也算进内容区。
 */
const splitLimits = computed(() => {
  const size = splitSize.value
  const available = size - splitGutter.value
  if (size <= 0 || available <= 0) {
    return { min: SPLIT_MIN_RATIO, max: SPLIT_MAX_RATIO }
  }
  const minByContainer = (size / 3) / available
  const min = Math.min(0.5, Math.max(SPLIT_MIN_RATIO, minByContainer))
  return { min, max: 1 - min }
})
const splitRequestRatio = computed(() =>
  Math.min(splitLimits.value.max, Math.max(splitLimits.value.min, ui.requestRatio)),
)

function setSplitRatio(ratio: number) {
  if (!Number.isFinite(ratio)) return
  const { min, max } = splitLimits.value
  ui.setRequestRatio(Math.min(max, Math.max(min, ratio)))
}

function readSplitMetrics() {
  const el = splitEl.value
  if (!el) return
  const vertical = ui.layout === 'side-by-side'
  const styles = window.getComputedStyle(el)
  const gutter = Number.parseFloat(vertical ? styles.columnGap : styles.rowGap)
  splitGutter.value = Number.isFinite(gutter) ? gutter : 3
  const rect = el.getBoundingClientRect()
  splitSize.value = vertical ? rect.width : rect.height
}

function observeSplit() {
  splitObserver?.disconnect()
  if (splitEl.value && splitObserver) splitObserver.observe(splitEl.value)
  readSplitMetrics()
}

function onSplitResizeStart(e: PointerEvent) {
  if (e.button !== 0 || !splitEl.value) return
  const bar = e.currentTarget as HTMLElement
  const split = splitEl.value
  const vertical = ui.layout === 'side-by-side'
  const rect = split.getBoundingClientRect()
  const start = vertical ? rect.left : rect.top
  const size = vertical ? rect.width : rect.height
  const gutter = splitGutter.value
  const available = Math.max(1, size - gutter)
  const startPosition = vertical ? e.clientX : e.clientY
  const requestPane = split.firstElementChild as HTMLElement | null
  const requestRect = requestPane?.getBoundingClientRect()
  const requestSize = requestRect
    ? (vertical ? requestRect.width : requestRect.height)
    : available * splitRequestRatio.value
  const dividerCenter = start + requestSize + gutter / 2
  // 命中区比视觉分隔条更宽，保留用户点击点与分隔条中心的偏移，避免首帧跳动。
  const grabOffset = startPosition - dividerCenter

  bar.setPointerCapture(e.pointerId)
  splitDragging.value = true

  const onMove = (ev: PointerEvent) => {
    const position = vertical ? ev.clientX : ev.clientY
    const ratio = (position - start - grabOffset - gutter / 2) / available
    setSplitRatio(ratio)
  }
  const onEnd = () => {
    splitDragging.value = false
    bar.removeEventListener('pointermove', onMove)
    bar.removeEventListener('pointerup', onEnd)
    bar.removeEventListener('pointercancel', onEnd)
  }

  bar.addEventListener('pointermove', onMove)
  bar.addEventListener('pointerup', onEnd)
  bar.addEventListener('pointercancel', onEnd)
  // 阻止拖动时选中文字，并让浏览器把后续事件交给分隔条。
  e.preventDefault()
}

/** 分隔条也支持键盘：方向键按 2% 调整，Shift 加速，Home/End 到边界。 */
function onSplitResizeKey(e: KeyboardEvent) {
  const vertical = ui.layout === 'side-by-side'
  const step = e.shiftKey ? 0.05 : 0.02
  let next: number | undefined

  if ((vertical && e.key === 'ArrowLeft') || (!vertical && e.key === 'ArrowUp')) {
    next = splitRequestRatio.value - step
  } else if ((vertical && e.key === 'ArrowRight') || (!vertical && e.key === 'ArrowDown')) {
    next = splitRequestRatio.value + step
  } else if (e.key === 'Home') {
    next = splitLimits.value.min
  } else if (e.key === 'End') {
    next = splitLimits.value.max
  } else if (e.key === 'Enter' || e.key === ' ') {
    next = SPLIT_DEFAULT_RATIO
  } else {
    return
  }

  setSplitRatio(next)
  e.preventDefault()
}

function resetSplitRatio() {
  setSplitRatio(SPLIT_DEFAULT_RATIO)
}

const reqSize = computed(() =>
  ui.layout === 'focus' ? '0 0 0' : `${splitRequestRatio.value} 1 0`,
)
const responseSize = computed(() =>
  ui.layout === 'focus' ? '1 1 0' : `${1 - splitRequestRatio.value} 1 0`,
)

watch(
  () => ui.layout,
  () => {
    // 每次切换方向都从真正的 50/50 开始，避免沿用另一方向的旧比例。
    ui.resetRequestRatio()
    void nextTick(observeSplit)
  },
)
watch(
  [() => ui.spikeOpen, () => activeNewPanel.value, () => activeKind.value],
  () => void nextTick(observeSplit),
)

function onKeydown(e: KeyboardEvent) {
  const mod = e.ctrlKey || e.metaKey
  if (!mod) return
  const k = e.key.toLowerCase()
  if (k === 'b') { e.preventDefault(); ui.sidebarVisible = !ui.sidebarVisible }
  // Ctrl+T 是 1 步操作（02 文档 §1 第 3 条），直接创建并激活“新建请求”标签。
  else if (k === 't') { e.preventDefault(); tabs.openQuick() }
  else if (k === 'k') { e.preventDefault(); ui.paletteOpen = true }
  else if (k === ',') { e.preventDefault(); tabs.openSettings() }
  else if (k === 'l' && e.shiftKey) { e.preventDefault(); ui.cycleLayout() }
  else if (e.shiftKey && k === 'arrowup') { e.preventDefault(); setSplitRatio(splitLimits.value.max) }
  else if (e.shiftKey && k === 'arrowdown') { e.preventDefault(); setSplitRatio(splitLimits.value.min) }
}
</script>

<template>
  <div
    class="app"
    :class="{
      resizing: dragging,
      'split-resizing': splitDragging,
      'split-col-resizing': splitDragging && ui.layout === 'side-by-side',
    }"
    tabindex="-1"
    @keydown="onKeydown"
  >
    <TitleBar />
    <div class="mid">
      <template v-if="ui.sidebarVisible && sidebarReady">
        <Sidebar class="a-block" :style="sidebarStyle" :nodes="workspace.nodes" />
        <!--
          宽度分隔条。role=separator + tabindex 让它成为一个可聚焦的
          「值可调的分隔符」，读屏会念出当前宽度与上下限。
          双击回默认宽度（240），这是拖拽类控件的通用约定。
        -->
        <div
          class="resizer"
          :class="{ dragging }"
          role="separator"
          aria-orientation="vertical"
          aria-label="调整侧边栏宽度"
          :aria-valuenow="ui.sidebarWidth"
          :aria-valuemin="SIDEBAR_MIN_W"
          :aria-valuemax="ui.sidebarMaxW"
          tabindex="0"
          title="拖动调整宽度，双击恢复默认"
          @pointerdown="onResizeStart"
          @keydown="onResizeKey"
          @dblclick="ui.resetSidebarWidth()"
        />
      </template>
      <WorkspaceArea>
        <template #toolbar>
          <UrlBar v-if="!ui.spikeOpen && activeKind === 'request' && !activeNewPanel" />
        </template>

        <div v-if="ui.spikeOpen" class="a-block spike">
          <div class="spike-head">
            <button type="button" @click="ui.spikeOpen = false">← 返回</button>
            <button type="button" :disabled="ui.spikePanel === 's2'" @click="ui.spikePanel = 's2'">S2</button>
            <button type="button" :disabled="ui.spikePanel === 's3'" @click="ui.spikePanel = 's3'">S3</button>
          </div>
          <S3Panel v-if="ui.spikePanel === 's3'" />
          <S2Panel v-else />
        </div>
        <SettingsPanel v-else-if="activeKind === 'settings'" />
        <div v-else-if="activeNewPanel" class="split new-content" :style="splitStyle">
          <NewPanel id="new-request-panel" class="pane a-block" />
        </div>
        <div v-else ref="splitEl" class="split" :style="splitStyle">
          <RequestPanel
            class="pane a-block"
            :class="{ collapsed: ui.layout === 'focus' }"
            :style="{ flex: reqSize }"
          />
          <div
            v-if="ui.layout !== 'focus'"
            class="split-resizer"
            :class="{
              vertical: ui.layout === 'side-by-side',
              horizontal: ui.layout !== 'side-by-side',
              dragging: splitDragging,
            }"
            role="separator"
            :aria-orientation="ui.layout === 'side-by-side' ? 'vertical' : 'horizontal'"
            aria-label="调整请求区与响应区大小"
            :aria-valuenow="Math.round(splitRequestRatio * 100)"
            :aria-valuemin="Math.round(splitLimits.min * 100)"
            :aria-valuemax="Math.round(splitLimits.max * 100)"
            :aria-valuetext="`请求区 ${Math.round(splitRequestRatio * 100)}%，响应区 ${Math.round((1 - splitRequestRatio) * 100)}%`"
            tabindex="0"
            title="拖动调整请求区与响应区大小，双击恢复默认"
            @pointerdown="onSplitResizeStart"
            @keydown="onSplitResizeKey"
            @dblclick="resetSplitRatio"
          />
          <ResponsePanel class="pane a-block" :style="{ flex: responseSize }" />
        </div>
      </WorkspaceArea>
    </div>
    <StatusBar />
    <HistoryPanel v-if="ui.historyOpen" />
    <CommandPalette v-if="ui.paletteOpen" />
  </div>
</template>

<style scoped>
.app { display: flex; flex-direction: column; height: 100%; outline: none; }
/*
 * 拖拽期间把整个窗口的光标钉成 col-resize。
 *
 * setPointerCapture 只改事件派发目标，光标形状还是由指针下方的元素决定 ——
 * 不钉住的话，拖过响应区就会变成文本光标，看着像拖拽已经断开了。
 * 这是唯一需要 !important 的地方：要盖掉子组件里各自声明的 cursor。
 */
.app.resizing,
.app.resizing :deep(*) { cursor: col-resize !important; }
.app.split-resizing,
.app.split-resizing :deep(*) { cursor: row-resize !important; }
.app.split-col-resizing,
.app.split-col-resizing :deep(*) { cursor: col-resize !important; }

/*
 * 内容区用「块 + 间隙」分隔（02 文档 §3.2 / §7.1）。
 *
 * 顶栏和状态栏仍然是通栏贴边的**装饰**（bg-2），中间这一片是**画布**（bg-1），
 * 内容块浮在画布上。层级读起来是「壳 → 画布 → 块」，靠底色差而不是描边分区。
 *
 * 间隙代价：垂直 4 条 × 3px = 12px，水平 3 条 × 3px = 9px。
 * 1280×820 下内容高度从 760px 降到 748px，少 1.6%。
 */
.mid {
  flex: 1 1 auto;
  display: flex;
  min-height: 0;
  gap: var(--a-gutter);
  padding: var(--a-gutter);
  background: var(--a-bg-1);
}
/*
 * 侧边栏与主区之间的宽度分隔条。
 *
 * 负外边距把它**盖在那条 3px 间隙上**，而不是插进去多占一格：
 * flex 的 gap 会在分隔条两侧各加一条 3px，直接放进去总分隔就变成 9px，
 * 破坏 §3.2 定的 3px 间隙。`margin: 0 -3px` 恰好抵掉多出来的两条。
 *
 * 命中区靠 ::before 撑到 7px（3px 太细，拖不住），且刻意**只往主区一侧多扩**
 * 3px、往侧边栏一侧只扩 1px（正好它的边框）：侧边栏右缘就是集合树的滚动条，
 * 往那边扩会抢走滚动条的抓取宽度。
 */
.resizer {
  flex: 0 0 auto;
  position: relative;
  z-index: 5;
  width: var(--a-gutter);
  margin: 0 calc(-1 * var(--a-gutter));
  cursor: col-resize;
  touch-action: none;
}
.resizer::before {
  content: '';
  position: absolute;
  inset: 0 -3px 0 -1px;
}
/* 高亮条只在悬停/拖拽/聚焦时出现。平时不画东西 —— 常驻一条竖线就等于把
   §3.2「不用 1px 细线分区」又加回来了。只动 opacity，符合 §7.4 的动效白名单。 */
.resizer::after {
  content: '';
  position: absolute;
  inset: 0;
  background: var(--a-primary);
  opacity: 0;
  transition: opacity var(--a-dur) var(--a-ease);
}
.resizer:hover::after,
.resizer:focus-visible::after,
.resizer.dragging::after { opacity: 1; }
/* 焦点环画在分隔条内侧：3px 宽的元素套 2px outline + 1px offset 会糊成一团。 */
.resizer:focus-visible { outline-offset: -1px; }

.split { flex: 1 1 auto; display: flex; min-height: 0; gap: var(--a-gutter); }
.split-resizer {
  flex: 0 0 auto;
  position: relative;
  z-index: 5;
  touch-action: none;
}
.split-resizer.vertical {
  width: var(--a-gutter);
  margin: 0 calc(-1 * var(--a-gutter));
  cursor: col-resize;
}
.split-resizer.horizontal {
  height: var(--a-gutter);
  margin: calc(-1 * var(--a-gutter)) 0;
  cursor: row-resize;
}
.split-resizer::before,
.split-resizer::after { content: ''; position: absolute; }
/* 命中区比视觉间隙更宽，避免必须精确点中 3px。 */
.split-resizer.vertical::before { inset: 0 -3px 0 -1px; }
.split-resizer.horizontal::before { inset: -1px 0 -3px 0; }
/* 平时不画常驻细线，只在交互时高亮间隙。 */
.split-resizer::after {
  inset: 0;
  background: var(--a-primary);
  opacity: 0;
  transition: opacity var(--a-dur) var(--a-ease);
}
.split-resizer:hover::after,
.split-resizer:focus-visible::after,
.split-resizer.dragging::after { opacity: 1; }
.split-resizer:focus-visible { outline-offset: -1px; }
.pane { min-height: 0; min-width: 0; display: flex; flex-direction: column; }
/*
 * 专注模式把请求区压到 flex: 0 0 0。块状布局下不能只压高度 ——
 * 1px 上下边框加一条 gutter 会留下一道 10px 的空缝，看着像渲染故障。
 * 直接不占位。
 */
.pane.collapsed { display: none; }

.spike { flex: 1 1 auto; display: flex; flex-direction: column; min-height: 0; }
.spike-head { flex: 0 0 auto; display: flex; gap: 6px; padding: 6px 8px; border-bottom: 1px solid var(--a-border); }
.spike-head button:disabled { opacity: 0.5; cursor: default; }
.spike-head button {
  height: 24px; padding: 0 10px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text);
  font: inherit; cursor: pointer;
}
.spike-head button:hover { background: var(--a-hover); }
</style>
