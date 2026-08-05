<script setup lang="ts">
import { defineAsyncComponent, computed, onMounted, onBeforeUnmount, ref } from 'vue'
import TitleBar from '@/components/layout/TitleBar.vue'
import TabBar from '@/components/layout/TabBar.vue'
import StatusBar from '@/components/layout/StatusBar.vue'
import UrlBar from '@/components/request/UrlBar.vue'
import RequestPanel from '@/components/request/RequestPanel.vue'
import ResponsePanel from '@/components/response/ResponsePanel.vue'
import { useUiStore, SIDEBAR_MIN_W } from '@/stores/ui'
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
const SettingsModal = defineAsyncComponent(() => import('@/views/SettingsModal.vue'))
const NewModal = defineAsyncComponent(() => import('@/views/NewModal.vue'))
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
onMounted(() => {
  onWindowResize()
  window.addEventListener('resize', onWindowResize)
})
onBeforeUnmount(() => window.removeEventListener('resize', onWindowResize))

const splitStyle = computed(() =>
  ui.layout === 'side-by-side'
    ? { flexDirection: 'row' as const }
    : { flexDirection: 'column' as const },
)
const reqSize = computed(() =>
  ui.layout === 'focus' ? '0 0 0' : `${ui.requestRatio} 1 0`,
)

function onKeydown(e: KeyboardEvent) {
  const mod = e.ctrlKey || e.metaKey
  if (!mod) return
  const k = e.key.toLowerCase()
  if (k === 'b') { e.preventDefault(); ui.sidebarVisible = !ui.sidebarVisible }
  // Ctrl+T 是 1 步操作（02 文档 §1 第 3 条），故意**不**经过新建面板。
  else if (k === 't') { e.preventDefault(); tabs.openQuick() }
  else if (k === 'k') { e.preventDefault(); ui.paletteOpen = true }
  else if (k === ',') { e.preventDefault(); ui.settingsOpen = true }
  else if (k === 'l' && e.shiftKey) { e.preventDefault(); ui.cycleLayout() }
}
</script>

<template>
  <div class="app" :class="{ resizing: dragging }" tabindex="-1" @keydown="onKeydown">
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
      <main class="main">
        <!--
          标签栏与 URL 栏合成一个「请求头」块。分成两块会让顶部出现三条圆角边，
          视觉上过碎；它们本来也是同一层语义（当前请求是谁 + 请求发到哪）。
        -->
        <div class="a-block head">
          <TabBar />
          <UrlBar v-if="!ui.spikeOpen" />
        </div>

        <div v-if="ui.spikeOpen" class="a-block spike">
          <div class="spike-head">
            <button type="button" @click="ui.spikeOpen = false">← 返回</button>
            <button type="button" :disabled="ui.spikePanel === 's2'" @click="ui.spikePanel = 's2'">S2</button>
            <button type="button" :disabled="ui.spikePanel === 's3'" @click="ui.spikePanel = 's3'">S3</button>
          </div>
          <S3Panel v-if="ui.spikePanel === 's3'" />
          <S2Panel v-else />
        </div>
        <div v-else class="split" :style="splitStyle">
          <RequestPanel
            class="pane a-block"
            :class="{ collapsed: ui.layout === 'focus' }"
            :style="{ flex: reqSize }"
          />
          <ResponsePanel class="pane a-block" style="flex: 1 1 0" />
        </div>
      </main>
    </div>
    <StatusBar />
    <SettingsModal v-if="ui.settingsOpen" />
    <NewModal v-if="ui.newOpen" />
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
.main {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  gap: var(--a-gutter);
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

.head { flex: 0 0 auto; display: flex; flex-direction: column; }
/*
 * 覆盖 .a-block 的 overflow: hidden —— URL 栏里方法选择器的下拉菜单要能溢出
 * 这个块（块只有 74px 高，菜单有 110px+），裁掉就等于菜单打不开。
 * 圆角改成传给首尾子元素，视觉效果一样但不裁剪弹层。
 */
.head { overflow: visible; }
.head > :first-child {
  border-top-left-radius: inherit;
  border-top-right-radius: inherit;
}
.head > :last-child {
  border-bottom-left-radius: inherit;
  border-bottom-right-radius: inherit;
}

.split { flex: 1 1 auto; display: flex; min-height: 0; gap: var(--a-gutter); }
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
