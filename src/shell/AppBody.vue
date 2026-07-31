<script setup lang="ts">
import { defineAsyncComponent, computed, onMounted, ref } from 'vue'
import TitleBar from '@/components/layout/TitleBar.vue'
import TabBar from '@/components/layout/TabBar.vue'
import StatusBar from '@/components/layout/StatusBar.vue'
import UrlBar from '@/components/request/UrlBar.vue'
import RequestPanel from '@/components/request/RequestPanel.vue'
import ResponsePanel from '@/components/response/ResponsePanel.vue'
import { useUiStore } from '@/stores/ui'
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
const S2Panel = defineAsyncComponent(() => import('@/spike/S2Panel.vue'))
const S3Panel = defineAsyncComponent(() => import('@/spike/S3Panel.vue'))

const ui = useUiStore()

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
  else if (k === 'k') { e.preventDefault(); ui.paletteOpen = true }
  else if (k === ',') { e.preventDefault(); ui.settingsOpen = true }
  else if (k === 'l' && e.shiftKey) { e.preventDefault(); ui.cycleLayout() }
}
</script>

<template>
  <div class="app" tabindex="-1" @keydown="onKeydown">
    <TitleBar />
    <div class="mid">
      <Sidebar v-if="ui.sidebarVisible && sidebarReady" class="a-block" />
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
  </div>
</template>

<style scoped>
.app { display: flex; flex-direction: column; height: 100%; outline: none; }

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
