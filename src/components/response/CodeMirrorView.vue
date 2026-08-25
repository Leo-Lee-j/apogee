<script setup lang="ts">
import { ref, shallowRef, computed, onMounted, onBeforeUnmount, watch } from 'vue'
import { fetchResponseRange, fetchSpikeResponseRange } from '@/ipc'
import { useUiStore } from '@/stores/ui'
import type { EditorView } from '@codemirror/view'

/**
 * 原文视图 —— Spike S2 的前端主体。
 *
 * # 设计要点（03 文档 §4.3）
 *
 * 原文模式通过 `apogee://` 自定义协议按 Range 拉首屏分片（默认 64KB），
 * 不在前端持有完整响应。只有小于 2MB 的 JSON 美化模式会读取完整内容，
 * 因为格式化需要解析完整文档，且调用侧会在阈值处禁用该模式。
 *
 * 用 CodeMirror 6 而不是 Monaco：体积小约一个数量级，且原生支持大文档与虚拟视口
 * （03 文档 §1.2 明确排除 Monaco，它会击穿 15MB 包体预算）。
 *
 * 高亮阈值 2MB（02 文档 §4.4）：超过就只保留行号，不做语法高亮。
 * 语法高亮的成本随文本长度线性上升，是大响应体卡顿的主要来源之一。
 */

const ui = useUiStore()

const props = defineProps<{
  responseId: string
  totalBytes: number
  highlight: boolean
  /** 小型 JSON 的美化视图；原文模式绝不改写响应内容。 */
  formatJson?: boolean
  /** 首屏拉取字节数。S2 会扫不同取值。 */
  firstChunkBytes?: number
  /** S2 的合成响应走显式 Synthetic scope；产品响应保持 workspace session scope。 */
  responseScope?: 'workspace' | 'spike'
}>()

type TextMode = 'raw' | 'pretty' | 'compact' | 'escaped'
const mode = ref<TextMode>(props.formatJson ? 'pretty' : 'raw')
const wrap = ref(true)

export interface CmMetrics {
  /** Range 请求耗时（IPC/协议层） */
  fetchMs: number
  /** CodeMirror 实例创建 + 首次布局 */
  editorMs: number
  /** 从开始加载到浏览器完成首帧绘制 */
  firstPaintMs: number
  /** 本次装入编辑器的字节数 */
  loadedBytes: number
  totalBytes: number
  highlight: boolean
  lightweight: boolean
  /** 装载后的 DOM 节点数，用于判断是否真的做了视口虚拟化 */
  domNodes: number
}

const emit = defineEmits<{
  metrics: [CmMetrics]
  /** 加载失败也必须报出去。只在 UI 上显示错误会让自动化量测只能等超时。 */
  failed: [string]
}>()

const host = ref<HTMLDivElement | null>(null)
const view = shallowRef<EditorView | null>(null)
const loading = ref(true)
const err = ref<string | null>(null)
const metrics = ref<CmMetrics | null>(null)
const loadedBytes = ref(0)

const transformed = computed(() => mode.value !== 'raw')
// 大文档的主要成本来自编辑器扩展维护的索引，而不是可见行本身。
// 超过 512 KB 时保留行号与换行，关闭折叠、搜索和活动行等高开销扩展。
const lightweight = computed(() => props.totalBytes > 512 * 1024)
const FIRST_CHUNK = () => transformed.value
  ? props.totalBytes
  : (props.firstChunkBytes ?? 64 * 1024)

async function mountEditor() {
  if (!host.value) return
  loading.value = true
  err.value = null
  const t0 = performance.now()

  try {
    // 动态 import：CodeMirror 不进首屏 chunk（03 文档 §4.1）
    const [{ EditorState }, { EditorView, lineNumbers, highlightActiveLine, keymap }, language, searchModule] = await Promise.all([
      import('@codemirror/state'),
      import('@codemirror/view'),
      lightweight.value ? Promise.resolve(null) : import('@codemirror/language'),
      lightweight.value ? Promise.resolve(null) : import('@codemirror/search'),
    ])

    const loadRange = props.responseScope === 'spike'
      ? fetchSpikeResponseRange
      : fetchResponseRange
    const chunk = await loadRange(props.responseId, 0, FIRST_CHUNK())
    loadedBytes.value = chunk.bytes
    const documentText = mode.value === 'pretty'
      ? JSON.stringify(JSON.parse(chunk.text), null, 2)
      : mode.value === 'compact'
        ? JSON.stringify(JSON.parse(chunk.text))
        : mode.value === 'escaped'
          ? JSON.stringify(chunk.text)
          : chunk.text

    const extensions: unknown[] = [
      lineNumbers(),
      ...(wrap.value ? [EditorView.lineWrapping] : []),
      EditorView.editable.of(false),
      EditorView.theme({
        '&': { height: '100%', fontSize: 'var(--a-fs-result)' },
        '.cm-scroller': { fontFamily: 'var(--a-font-mono)', lineHeight: 'var(--a-lh-result)' },
        '.cm-gutters': { background: 'var(--a-bg-2)', border: 'none', color: 'var(--a-text-faint)' },
        '.cm-activeLine': { background: 'var(--a-hover)' },
        '.cm-panels': { backgroundColor: 'var(--a-bg-2)', color: 'var(--a-text)', borderBottom: '1px solid var(--a-border)' },
        '.cm-panels input, .cm-panels button': { fontFamily: 'var(--a-font-ui)', fontSize: 'var(--a-fs-sm)', lineHeight: 'var(--a-lh-sm)' },
        '.cm-search label': { display: 'inline-flex', alignItems: 'center', gap: '4px', minHeight: 'var(--a-h-control-xs)', color: 'var(--a-text-dim)', lineHeight: 'var(--a-lh-xs)' },
        '.cm-textfield': { height: 'var(--a-h-control-xs)', padding: '0 6px', backgroundColor: 'var(--a-bg-3)', color: 'var(--a-text)', border: '1px solid var(--a-border)', borderRadius: 'var(--a-radius-sm)', outline: 'none' },
        '.cm-textfield:focus': { borderColor: 'var(--a-focus)', boxShadow: 'var(--a-focus-halo)' },
        '.cm-button': { appearance: 'none', height: 'var(--a-h-control-xs)', padding: '0 8px', backgroundColor: 'var(--a-bg-3)', backgroundImage: 'none', color: 'var(--a-text-soft)', border: '1px solid var(--a-border)', borderRadius: 'var(--a-radius-sm)', boxShadow: 'none', lineHeight: 'var(--a-lh-xs)' },
        '.cm-button:hover': { backgroundColor: 'var(--a-hover)', color: 'var(--a-text)' },
        '.cm-search input[type="checkbox"]': { width: '14px', height: '14px', margin: '0', accentColor: 'var(--a-primary)', verticalAlign: 'middle' },
        '.cm-searchMatch': { backgroundColor: 'var(--a-selected)', outline: '1px solid var(--a-primary)' },
        '.cm-selectionMatch': { backgroundColor: 'var(--a-hover)' },
      }),
    ]

    if (!lightweight.value && language && searchModule) {
      const { foldGutter, foldKeymap, foldAll, unfoldAll } = language
      const { search, searchKeymap, closeSearchPanel } = searchModule
      extensions.push(
        highlightActiveLine(),
        foldGutter(),
        search(),
        keymap.of([
          // searchKeymap 提供 Ctrl/Cmd+F；Escape 会关闭已打开的搜索面板。
          ...searchKeymap,
          { key: 'Escape', run: closeSearchPanel, preventDefault: true },
          ...foldKeymap,
          { key: 'Mod-Alt-0', run: foldAll },
          { key: 'Mod-Alt-9', run: unfoldAll },
        ]),
      )
    }

    // 只有在阈值以内才加载语言支持与语法高亮。
    // 这一步是分级降级的实际落点，不是配置项。
    if (props.highlight && !lightweight.value) {
      const [{ json }, { syntaxHighlighting, defaultHighlightStyle }] = await Promise.all([
        import('@codemirror/lang-json'),
        import('@codemirror/language'),
      ])
      extensions.push(json(), syntaxHighlighting(defaultHighlightStyle, { fallback: true }))
    }

    const tEditor = performance.now()
    const v = new EditorView({
      state: EditorState.create({ doc: documentText, extensions: extensions as never }),
      parent: host.value,
    })
    view.value = v
    const editorMs = performance.now() - tEditor

    loading.value = false

    // 等浏览器真正画完一帧再记时间。
    // 只测到 new EditorView() 返回是不诚实的 —— 那时候像素还没上屏。
    await new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())))
    const firstPaintMs = performance.now() - t0

    const m: CmMetrics = {
      fetchMs: chunk.fetchMs,
      editorMs,
      firstPaintMs,
      loadedBytes: chunk.bytes,
      totalBytes: chunk.totalBytes || props.totalBytes,
      highlight: props.highlight,
      lightweight: lightweight.value,
      domNodes: host.value?.querySelectorAll('*').length ?? 0,
    }
    metrics.value = m
    emit('metrics', m)
  } catch (e) {
    loading.value = false
    err.value = e instanceof Error ? e.message : String(e)
    emit('failed', err.value)
  }
}

function destroyEditor() {
  view.value?.destroy()
  view.value = null
}

function refresh() {
  destroyEditor()
  if (host.value) host.value.innerHTML = ''
  void mountEditor()
}

function setMode(next: TextMode) {
  mode.value = next
  refresh()
}

function toggleWrap() {
  wrap.value = !wrap.value
  refresh()
}

async function searchText() {
  const currentView = view.value
  if (!currentView || lightweight.value) return
  const { openSearchPanel } = await import('@codemirror/search')
  if (view.value === currentView) openSearchPanel(currentView)
}

async function closeSearch() {
  const currentView = view.value
  if (!currentView || lightweight.value) return
  const { closeSearchPanel, searchPanelOpen } = await import('@codemirror/search')
  if (view.value === currentView && searchPanelOpen(currentView.state)) {
    closeSearchPanel(currentView)
  }
}

function handleSearchKeydown(event: KeyboardEvent) {
  if (!view.value || lightweight.value) return

  const isFindShortcut = (event.ctrlKey || event.metaKey)
    && !event.altKey
    && event.key.toLowerCase() === 'f'

  if (isFindShortcut) {
    // 必须在同步事件阶段阻止默认行为，否则浏览器会先打开页面搜索。
    event.preventDefault()
    event.stopPropagation()
    void searchText()
  } else if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    void closeSearch()
  }
}

async function toggleFolds(all: boolean) {
  if (!view.value || lightweight.value) return
  const { foldAll, unfoldAll } = await import('@codemirror/language')
  ;(all ? foldAll : unfoldAll)(view.value)
}

onMounted(mountEditor)
onBeforeUnmount(destroyEditor)
watch(() => [props.responseId, props.highlight, props.formatJson], () => {
  mode.value = props.formatJson ? 'pretty' : 'raw'
  refresh()
})
// CSS 变量会即时改变字形，但 CodeMirror 的视口高度缓存需要显式重测。
watch(() => ui.resultFontSize, () => {
  requestAnimationFrame(() => view.value?.requestMeasure())
})

function canSearch() {
  return !lightweight.value && view.value !== null
}

defineExpose({ metrics, canSearch, openSearch: searchText, closeSearch })
</script>

<template>
  <div
    class="cm-wrap"
    role="region"
    aria-label="响应文本结果"
    tabindex="0"
    @keydown.capture="handleSearchKeydown"
  >
    <div v-if="mode !== 'raw'" class="tools" role="toolbar" aria-label="响应文本工具">
      <button type="button" :class="{ on: mode === 'pretty' }" @click="setMode('pretty')">美化</button>
      <button type="button" :class="{ on: mode === 'compact' }" @click="setMode('compact')">压缩</button>
      <button type="button" :class="{ on: mode === 'escaped' }" @click="setMode('escaped')">转义</button>
      <button type="button" :class="{ on: wrap }" @click="toggleWrap">换行</button>
      <template v-if="!lightweight">
        <button type="button" @click="toggleFolds(true)">折叠</button>
        <button type="button" @click="toggleFolds(false)">展开</button>
        <button type="button" @click="searchText">搜索</button>
      </template>
    </div>
    <div v-if="loading" class="state">加载首屏分片…</div>
    <div v-else-if="err" class="state err">
      <p>{{ formatJson ? 'JSON 美化失败' : '原文加载失败' }}：{{ err }}</p>
      <p class="dim">这通常意味着自定义协议未注册，或 Range 头未被正确处理。</p>
    </div>
    <div ref="host" class="cm-host" />
    <div v-if="metrics" class="foot">
      已装入 {{ (metrics.loadedBytes / 1024).toFixed(0) }} KB / 共
      {{ (metrics.totalBytes / 1048576).toFixed(2) }} MB
      · 取分片 {{ metrics.fetchMs.toFixed(1) }}ms
      · 建编辑器 {{ metrics.editorMs.toFixed(1) }}ms
      · 首帧 {{ metrics.firstPaintMs.toFixed(1) }}ms
      · DOM {{ metrics.domNodes }} 节点
      · {{ metrics.lightweight ? '轻量模式' : `高亮 ${metrics.highlight ? '开' : '关'}` }}
    </div>
  </div>
</template>

<style scoped>
.cm-wrap { display: flex; flex-direction: column; min-height: 0; flex: 1 1 auto; }
.cm-wrap:focus-visible { outline: 1px solid var(--a-focus); outline-offset: -1px; }
.tools { flex: 0 0 auto; display: flex; align-items: center; gap: 2px; min-height: var(--a-h-control-sm); padding: 3px 8px; border-bottom: 1px solid var(--a-border); background: var(--a-bg-2); }
.tools button { height: var(--a-h-control-xs); padding: 0 7px; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-ui); cursor: pointer; }
.tools button:hover { background: var(--a-hover); color: var(--a-text); }
.tools button.on { background: var(--a-selected); color: var(--a-primary); }
.cm-host { flex: 1 1 auto; min-height: 0; overflow: hidden; }
.state { padding: 12px; font-size: var(--a-fs-sm); color: var(--a-text-dim); }
.state.err { color: var(--a-error); }
.dim { color: var(--a-text-faint); }
.foot {
  flex: 0 0 auto; padding: 3px 8px;
  border-top: 1px solid var(--a-border);
  background: var(--a-bg-2);
  font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); color: var(--a-text-faint);
}
</style>
