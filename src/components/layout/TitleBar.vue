<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { useUiStore } from '@/stores/ui'
import { windowControls, type Unlisten } from '@/ipc'

/**
 * 顶栏，36px（02 文档 §3.2）。
 * Windows 下与系统标题栏融合（自定义 titlebar），省掉一条 30px 的浪费。
 *
 * 因为 `decorations: false` 关掉了系统标题栏，最小化 / 最大化 / 关闭三个按钮
 * 必须自己画。它们贴在右上角且占满 36px 全高 —— 这不是审美选择：Windows 用户
 * 会把鼠标「甩」到屏幕角落点关闭，按钮不贴边就接不住这个动作。
 */
const ui = useUiStore()

/** 最大化状态只用来决定中间那个按钮画「最大化」还是「还原」。 */
const maximized = ref(false)
let stopWatching: Unlisten | undefined

onMounted(async () => {
  if (!windowControls.available) return
  maximized.value = await windowControls.isMaximized()
  // 用户还能通过拖到顶边、双击拖拽区、Win+↑ 最大化，这些都不经过我们的按钮，
  // 所以图标状态必须订阅真实窗口状态，不能只在点击时本地翻转。
  stopWatching = await windowControls.onMaximizeChange((v) => {
    maximized.value = v
  })
})

onBeforeUnmount(() => stopWatching?.())
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>
    <span class="brand">Apogee</span>

    <button class="chip" type="button" aria-label="切换工作区">
      工作区: my-api <span class="caret">▾</span>
    </button>
    <button class="chip" type="button" aria-label="切换环境">
      环境: dev <span class="caret">▾</span>
    </button>

    <button class="palette" type="button" @click="ui.paletteOpen = true">
      <span>搜索或输入命令</span>
      <kbd>Ctrl</kbd><kbd>K</kbd>
    </button>

    <div class="spacer" />

    <button class="icon" type="button" title="Spike 面板" @click="ui.spikeOpen = true">S</button>
    <button class="icon" type="button" title="设置" @click="ui.settingsOpen = true">⚙</button>
    <button
      class="icon"
      type="button"
      title="切换主题"
      @click="ui.themeMode = ui.isDark ? 'light' : 'dark'; ui.applyTheme()"
    >
      ◐
    </button>

    <!--
      窗口控制。图标用内联 SVG 而不是 Segoe Fluent Icons 字形（02 文档 §7.5 第 6 条）：
      字形方案依赖 Windows 专有字体，macOS 上会渲染成豆腐块，而这几个图标一共
      只有几条直线，内联比引字体省。
      坐标带 .5 偏移是为了让 1px 描边落在像素网格上，不然会发虚。
    -->
    <div v-if="windowControls.available" class="wincontrols">
      <button class="wc" type="button" title="最小化" aria-label="最小化" @click="windowControls.minimize()">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <path d="M.5 5.5h9" />
        </svg>
      </button>

      <button
        class="wc"
        type="button"
        :title="maximized ? '向下还原' : '最大化'"
        :aria-label="maximized ? '向下还原' : '最大化'"
        @click="windowControls.toggleMaximize()"
      >
        <svg v-if="maximized" viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <path d="M2.5 2.5V.5h7v7h-2" />
          <rect x=".5" y="2.5" width="7" height="7" />
        </svg>
        <svg v-else viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <rect x=".5" y=".5" width="9" height="9" />
        </svg>
      </button>

      <button class="wc close" type="button" title="关闭" aria-label="关闭" @click="windowControls.close()">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <path d="M.7.7l8.6 8.6M9.3.7L.7 9.3" />
        </svg>
      </button>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  height: var(--a-h-titlebar);
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 8px;
  background: var(--a-bg-2);
  border-bottom: 1px solid var(--a-border);
}
.brand { font-weight: 600; letter-spacing: 0.02em; padding: 0 4px; }
.chip,
.icon,
.palette {
  background: transparent;
  border: 1px solid transparent;
  color: var(--a-text-dim);
  font: inherit;
  height: 24px;
  border-radius: 4px;
  cursor: pointer;
  transition: background var(--a-dur) var(--a-ease);
}
.chip { padding: 0 8px; }
.chip:hover, .icon:hover, .palette:hover { background: var(--a-hover); color: var(--a-text); }
.caret { opacity: 0.6; }
.palette {
  display: flex; align-items: center; gap: 6px;
  border-color: var(--a-border);
  padding: 0 8px; min-width: 220px;
  font-size: var(--a-fs-sm);
}
.palette kbd {
  font: var(--a-fs-xs) / 1 var(--a-font-mono);
  background: var(--a-bg-1); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm); padding: 2px 4px;
}
.spacer { flex: 1 1 auto; }
.icon { width: 24px; text-align: center; }

/* ── 窗口控制 ──
   负 margin 抵掉 .titlebar 的右侧 padding，让按钮真正贴到窗口右上角。
   宽 46px / 占满全高，对齐 Windows 11 的原生尺寸。 */
.wincontrols {
  display: flex;
  align-self: stretch;
  margin-left: 4px;
  margin-right: -8px;
}
.wc {
  width: 46px;
  align-self: stretch;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  border-radius: 0;
  color: var(--a-text-dim);
  cursor: pointer;
  transition: background var(--a-dur) var(--a-ease);
}
.wc svg {
  fill: none;
  stroke: currentcolor;
  stroke-width: 1;
}
.wc:hover { background: var(--a-hover); color: var(--a-text); }
.wc:active { background: var(--a-selected); }
/* 关闭按钮悬停用实色危险填充。这是唯一一个「悬停就变色块」的按钮，
   因为它是标题栏里唯一不可撤销的动作。 */
.wc.close:hover,
.wc.close:active {
  background: var(--a-danger-bg);
  color: var(--a-danger);
}
/* 按钮贴着窗口边缘，焦点环画在外侧会被窗口裁掉，所以收到内侧。 */
.wc:focus-visible { outline-offset: -2px; }
</style>
