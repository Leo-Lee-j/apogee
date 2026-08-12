<script setup lang="ts">
import { ref, shallowRef, computed, onMounted, onBeforeUnmount, watch } from 'vue'
import { fetchResponseRange } from '@/ipc'
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

const props = defineProps<{
  responseId: string
  totalBytes: number
  highlight: boolean
  /** 小型 JSON 的美化视图；原文模式绝不改写响应内容。 */
  formatJson?: boolean
  /** 首屏拉取字节数。S2 会扫不同取值。 */
  firstChunkBytes?: number
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
const lastText = ref('')

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

    const chunk = await fetchResponseRange(props.responseId, 0, FIRST_CHUNK())
    loadedBytes.value = chunk.bytes
    const documentText = mode.value === 'pretty'
      ? JSON.stringify(JSON.parse(chunk.text), null, 2)
      : mode.value === 'compact'
        ? JSON.stringify(JSON.parse(chunk.text))
        : mode.value === 'escaped'
          ? JSON.stringify(chunk.text)
          : chunk.text
    lastText.value = documentText

    const extensions: unknown[] = [
      lineNumbers(),
      ...(wrap.value ? [EditorView.lineWrapping] : []),
      EditorView.editable.of(false),
      EditorView.theme({
        '&': { height: '100%', fontSize: 'var(--a-font-result)' },
        '.cm-scroller': { fontFamily: 'var(--a-font-mono)', lineHeight: 'var(--a-lh-result)' },
        '.cm-gutters': { background: 'var(--a-bg-2)', border: 'none', color: 'var(--a-text-faint)' },
        '.cm-activeLine': { background: 'var(--a-hover)' },
        '.cm-panels': { backgroundColor: 'var(--a-bg-2)', color: 'var(--a-text)', borderBottom: '1px solid var(--a-border)' },
        '.cm-panels input, .cm-panels button': { fontFamily: 'var(--a-font-ui)', fontSize: '12px' },
        '.cm-search label': { display: 'inline-flex', alignItems: 'center', gap: '4px', color: 'var(--a-text-dim)', lineHeight: '24px' },
        '.cm-textfield': { height: '24px', padding: '0 6px', backgroundColor: 'var(--a-bg-3)', color: 'var(--a-text)', border: '1px solid var(--a-border)', borderRadius: 'var(--a-radius-sm)', outline: 'none' },
        '.cm-textfield:focus': { borderColor: 'var(--a-focus)', boxShadow: 'var(--a-focus-halo)' },
        '.cm-button': { appearance: 'none', height: '24px', padding: '0 8px', backgroundColor: 'var(--a-bg-3)', backgroundImage: 'none', color: 'var(--a-text-soft)', border: '1px solid var(--a-border)', borderRadius: 'var(--a-radius-sm)', boxShadow: 'none' },
        '.cm-button:hover': { backgroundColor: 'var(--a-hover)', color: 'var(--a-text)' },
        '.cm-search input[type="checkbox"]': { width: '14px', height: '14px', margin: '0', accentColor: 'var(--a-primary)', verticalAlign: 'middle' },
        '.cm-searchMatch': { backgroundColor: 'var(--a-selected)', outline: '1px solid var(--a-primary)' },
        '.cm-selectionMatch': { backgroundColor: 'var(--a-hover)' },
      }),
    ]

    if (!lightweight.value && language && searchModule) {
      const { foldGutter, foldKeymap, foldAll, unfoldAll } = language
      extensions.push(
        highlightActiveLine(),
        foldGutter(),
        searchModule.search(),
        keymap.of([...foldKeymap, { key: 'Mod-Alt-0', run: foldAll }, { key: 'Mod-Alt-9', run: unfoldAll }]),
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
  if (!view.value || lightweight.value) return
  const { openSearchPanel } = await import('@codemirror/search')
  openSearchPanel(view.value)
}

async function toggleFolds(all: boolean) {
  if (!view.value || lightweight.value) return
  const { foldAll, unfoldAll } = await import('@codemirror/language')
  ;(all ? foldAll : unfoldAll)(view.value)
}

function download() {
  if (loadedBytes.value !== (props.totalBytes || loadedBytes.value)) {
    err.value = '大响应请使用原文视图分片查看；完整下载将在文件保存接口接入后提供。'
    return
  }
  const blob = new Blob([lastText.value], { type: 'application/json;charset=utf-8' })
  const link = document.createElement('a')
  link.href = URL.createObjectURL(blob)
  link.download = `response-${props.responseId}.${mode.value === 'escaped' ? 'txt' : 'json'}`
  link.click()
  URL.revokeObjectURL(link.href)
}

onMounted(mountEditor)
onBeforeUnmount(destroyEditor)
watch(() => [props.responseId, props.highlight, props.formatJson], () => {
  mode.value = props.formatJson ? 'pretty' : 'raw'
  refresh()
})

defineExpose({ metrics })
</script>

<template>
  <div class="cm-wrap">
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
      <button type="button" @click="download">下载</button>
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
.tools { flex: 0 0 auto; display: flex; align-items: center; gap: 2px; min-height: 30px; padding: 3px 8px; border-bottom: 1px solid var(--a-border); background: var(--a-bg-2); }
.tools button { height: 24px; padding: 0 7px; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); font: 11px var(--a-font-ui); cursor: pointer; }
.tools button:hover { background: var(--a-hover); color: var(--a-text); }
.tools button.on { background: var(--a-selected); color: var(--a-primary); }
.cm-host { flex: 1 1 auto; min-height: 0; overflow: hidden; }
.state { padding: 12px; font-size: 12px; color: var(--a-text-dim); }
.state.err { color: var(--a-error); }
.dim { color: var(--a-text-faint); }
.foot {
  flex: 0 0 auto; padding: 3px 8px;
  border-top: 1px solid var(--a-border);
  background: var(--a-bg-2);
  font: 11px/16px var(--a-font-mono); color: var(--a-text-faint);
}
</style>
