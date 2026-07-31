<script setup lang="ts">
import { ref, shallowRef, onMounted, onBeforeUnmount, watch } from 'vue'
import { fetchResponseRange } from '@/ipc'

/**
 * 原文视图 —— Spike S2 的前端主体。
 *
 * # 设计要点（03 文档 §4.3）
 *
 * 前端**永远不持有完整响应字符串**。这里只做两件事：
 *   1. 通过 `apogee://` 自定义协议按 Range 拉首屏那一段（默认 64KB）
 *   2. 把这一段喂给 CodeMirror
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
  /** 首屏拉取字节数。S2 会扫不同取值。 */
  firstChunkBytes?: number
}>()

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
  /** 装载后的 DOM 节点数，用于判断是否真的做了视口虚拟化 */
  domNodes: number
}

const emit = defineEmits<{
  metrics: [CmMetrics]
  /** 加载失败也必须报出去。只在 UI 上显示错误会让自动化量测只能等超时。 */
  failed: [string]
}>()

const host = ref<HTMLDivElement | null>(null)
const view = shallowRef<{ destroy(): void } | null>(null)
const loading = ref(true)
const err = ref<string | null>(null)
const metrics = ref<CmMetrics | null>(null)
const loadedBytes = ref(0)

const FIRST_CHUNK = () => props.firstChunkBytes ?? 64 * 1024

async function mountEditor() {
  if (!host.value) return
  loading.value = true
  err.value = null
  const t0 = performance.now()

  try {
    // 动态 import：CodeMirror 不进首屏 chunk（03 文档 §4.1）
    const [{ EditorState }, { EditorView, lineNumbers, highlightActiveLine }] = await Promise.all([
      import('@codemirror/state'),
      import('@codemirror/view'),
    ])

    const chunk = await fetchResponseRange(props.responseId, 0, FIRST_CHUNK())
    loadedBytes.value = chunk.bytes

    const extensions: unknown[] = [
      lineNumbers(),
      highlightActiveLine(),
      EditorView.editable.of(false),
      EditorView.theme({
        '&': { height: '100%', fontSize: '13px' },
        '.cm-scroller': { fontFamily: 'var(--a-font-mono)', lineHeight: '20px' },
        '.cm-gutters': { background: 'var(--a-bg-2)', border: 'none', color: 'var(--a-text-faint)' },
        '.cm-activeLine': { background: 'var(--a-hover)' },
      }),
    ]

    // 只有在阈值以内才加载语言支持与语法高亮。
    // 这一步是分级降级的实际落点，不是配置项。
    if (props.highlight) {
      const [{ json }, { syntaxHighlighting, defaultHighlightStyle }] = await Promise.all([
        import('@codemirror/lang-json'),
        import('@codemirror/language'),
      ])
      extensions.push(json(), syntaxHighlighting(defaultHighlightStyle, { fallback: true }))
    }

    const tEditor = performance.now()
    const v = new EditorView({
      state: EditorState.create({ doc: chunk.text, extensions: extensions as never }),
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

onMounted(mountEditor)
onBeforeUnmount(destroyEditor)
watch(() => [props.responseId, props.highlight], () => {
  destroyEditor()
  if (host.value) host.value.innerHTML = ''
  void mountEditor()
})

defineExpose({ metrics })
</script>

<template>
  <div class="cm-wrap">
    <div v-if="loading" class="state">加载首屏分片…</div>
    <div v-else-if="err" class="state err">
      <p>原文加载失败：{{ err }}</p>
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
      · 高亮 {{ metrics.highlight ? '开' : '关' }}
    </div>
  </div>
</template>

<style scoped>
.cm-wrap { display: flex; flex-direction: column; min-height: 0; flex: 1 1 auto; }
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
