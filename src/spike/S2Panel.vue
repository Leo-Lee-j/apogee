<script setup lang="ts">
import { ref, shallowRef, nextTick } from 'vue'
import { ipc, inTauri, type ResponseSummary } from '@/ipc'
import CodeMirrorView, { type CmMetrics } from '@/components/response/CodeMirrorView.vue'

/**
 * # Spike S2 量测台
 *
 * ## 要验证的
 *
 * 03 文档 §10 的 S2：
 * > 自定义协议传 10MB / 100MB body 到 CodeMirror。
 * > 成功标准：10MB 首屏 < 300ms，100MB 不崩。
 * > 失败预案：改为分片文件 + Blob URL 方案。
 *
 * ## 为什么必须在真机 WebView 里测
 *
 * 这条链路的瓶颈分布在三个地方，只有跑起来才知道各占多少：
 *   1. Rust 侧生成 + 落盘（磁盘 IO）
 *   2. 自定义协议 + Range 把分片送进 WebView（WebView2 的协议实现）
 *   3. CodeMirror 建实例 + 首帧绘制（渲染）
 *
 * 所以量测拆成这三段分别记时，而不是只看一个总数 —— 总数超了也得知道该优化哪。
 *
 * ## 关键对照
 *
 * 同时测「首屏只拉 64KB」和「把整个 body 拉进来」两种做法。
 * 后者是竞品的实际行为，用来量化「按 Range 只取首屏」到底省了多少。
 */

const SIZES = [
  { label: '1 MB', bytes: 1 * 1024 * 1024 },
  { label: '10 MB', bytes: 10 * 1024 * 1024 },
  { label: '50 MB', bytes: 50 * 1024 * 1024 },
  { label: '100 MB', bytes: 100 * 1024 * 1024 },
] as const

const FIRST_CHUNK = 64 * 1024

interface Row {
  size: string
  targetBytes: number
  /** Rust 侧生成 + 落盘 */
  genMs: number
  actualBytes: number
  tier: string
  highlight: boolean
  /** 只拉首屏 64KB */
  rangeFetchMs: number
  editorMs: number
  firstPaintMs: number
  domNodes: number
  /** 对照：整体拉取全部字节的耗时（模拟竞品做法） */
  fullFetchMs: number | null
  fullFetchBytes: number | null
  /** JSON SpanIndex 构建耗时与固定元数据规模 */
  indexMs: number | null
  totalNodes: number | null
  indexBytes: number | null
  /** SpanIndex 自报元数据 / body 体积；不受进程分配器与 mmap 工作集噪声影响。 */
  indexMetadataRatio: number | null
  /** 树视图取根节点一页 */
  treePageMs: number | null
  /** 协议自检结果 */
  proto: ProtoProbe | null
  /** 建索引前后的 Rust 进程私有内存（MB），用于量化 JSON 索引的内存代价 */
  memBeforeMb: number | null
  memAfterMb: number | null
  indexMemCostMb: number | null
  /** 索引内存 / body 体积 的倍数。这个比值决定分级阈值定在哪。 */
  indexMemRatio: number | null
  /** 若按分级规则跳过建索引，记录原因 */
  indexSkipped: string | null
  verdict: string
  error: string | null
}

const rows = ref<Row[]>([])
const running = ref(false)
const stage = ref('')
const done = ref(false)
const savedTo = ref<string | null>(null)

// 当前挂载在量测位上的响应，用来驱动 CodeMirrorView
const probe = shallowRef<{ id: string; bytes: number; highlight: boolean } | null>(null)
let settle: { ok: (m: CmMetrics) => void; fail: (e: Error) => void } | null = null

function onMetrics(m: CmMetrics) {
  settle?.ok(m)
  settle = null
}
/** 关键：失败要立刻结束等待，不能靠超时。否则一个协议错误要拖 30s×4。 */
function onFailed(msg: string) {
  settle?.fail(new Error(`编辑器加载失败：${msg}`))
  settle = null
}

/** 挂载 CodeMirrorView 并等它报出首帧量测。 */
function measureEditor(id: string, bytes: number, highlight: boolean): Promise<CmMetrics> {
  return new Promise<CmMetrics>((resolve, reject) => {
    settle = { ok: resolve, fail: reject }
    probe.value = { id, bytes, highlight }
    setTimeout(() => {
      if (settle) {
        settle = null
        reject(new Error('编辑器量测超时（30s）—— 视为该体积下渲染不可用'))
      }
    }, 30_000)
  })
}

export interface ProtoProbe {
  url: string
  ok: boolean
  status: number | null
  statusText: string
  contentRange: string | null
  acceptRanges: string | null
  bytes: number | null
  error: string | null
}

/**
 * 协议自检 —— 在跑正式量测之前先确认 `apogee://` 通不通。
 *
 * 第一轮 S2 就是因为没有这一步：协议层出错时 CodeMirror 只在界面上显示错误，
 * 量测台只能看到「超时」，拿不到根因。诊断信息要在最靠近失败点的地方取。
 */
async function probeProtocol(responseId: string): Promise<ProtoProbe> {
  const { responseUrl } = await import('@/ipc')
  const url = responseUrl(responseId)
  try {
    const res = await fetch(url, { headers: { Range: 'bytes=0-1023' } })
    const buf = await res.arrayBuffer()
    return {
      url,
      ok: res.ok,
      status: res.status,
      statusText: res.statusText,
      contentRange: res.headers.get('content-range'),
      acceptRanges: res.headers.get('accept-ranges'),
      bytes: buf.byteLength,
      error: null,
    }
  } catch (e) {
    return {
      url,
      ok: false,
      status: null,
      statusText: '',
      contentRange: null,
      acceptRanges: null,
      bytes: null,
      error: e instanceof Error ? `${e.name}: ${e.message}` : String(e),
    }
  }
}

/** 对照组体积上限。超过就跳过 —— 把 100MB 拼成 JS 字符串会拖死量测台本身。 */
const FULL_FETCH_CAP = 50 * 1024 * 1024

/**
 * 对照组：把**整个** body 取到 JS 侧并拼成字符串，量化「不做 Range」的代价。
 *
 * 这正是竞品的实际行为 —— 整体读进内存再交给编辑器。
 * 注意不能只发一个无 Range 的请求：我们的协议 handler 对无 Range 请求硬限了 4MB
 * （防误用），所以这里用连续 Range 把全量取回来，才是诚实的对照。
 */
async function measureFullFetch(
  id: string,
  totalBytes: number,
): Promise<{ ms: number; bytes: number; chars: number }> {
  const { fetchResponseRange } = await import('@/ipc')
  const STEP = 4 * 1024 * 1024
  const t = performance.now()
  let off = 0
  let bytes = 0
  const parts: string[] = []
  while (off < totalBytes) {
    const chunk = await fetchResponseRange(id, off, STEP)
    if (chunk.bytes === 0) break
    parts.push(chunk.text)
    bytes += chunk.bytes
    off += chunk.bytes
  }
  // 拼成单个字符串 —— 这一步是竞品真正的开销所在，不能省略
  const joined = parts.join('')
  return { ms: performance.now() - t, bytes, chars: joined.length }
}

async function runAll() {
  if (running.value) return
  running.value = true
  done.value = false
  rows.value = []

  for (const s of SIZES) {
    const row: Row = {
      size: s.label,
      targetBytes: s.bytes,
      genMs: 0, actualBytes: 0, tier: '', highlight: false,
      rangeFetchMs: 0, editorMs: 0, firstPaintMs: 0, domNodes: 0,
      fullFetchMs: null, fullFetchBytes: null,
      indexMs: null, totalNodes: null, indexBytes: null, indexMetadataRatio: null,
      treePageMs: null,
      proto: null,
      memBeforeMb: null, memAfterMb: null, indexMemCostMb: null, indexMemRatio: null,
      indexSkipped: null,
      verdict: '', error: null,
    }

    try {
      stage.value = `${s.label}：Rust 侧生成并落盘`
      const t0 = performance.now()
      const meta: ResponseSummary = await ipc.spikeMakeResponse(s.bytes)
      row.genMs = performance.now() - t0
      row.actualBytes = meta.sizeBytes
      row.tier = meta.tier
      row.highlight = meta.syntaxHighlight

      // 先自检协议，拿到确切的失败原因（而不是等编辑器超时）
      stage.value = `${s.label}：自检 apogee:// 协议`
      const pp = await probeProtocol(meta.id)
      row.proto = pp
      if (!pp.ok) {
        throw new Error(
          `协议自检失败：${pp.error ?? `HTTP ${pp.status} ${pp.statusText}`}（URL: ${pp.url}）`,
        )
      }

      stage.value = `${s.label}：Range 拉首屏 ${FIRST_CHUNK / 1024}KB 并渲染`
      await nextTick()
      const m = await measureEditor(meta.id, meta.sizeBytes, meta.syntaxHighlight)
      row.rangeFetchMs = m.fetchMs
      row.editorMs = m.editorMs
      row.firstPaintMs = m.firstPaintMs
      row.domNodes = m.domNodes

      // 卸载编辑器，避免多个实例叠加影响后续量测
      probe.value = null
      await nextTick()

      if (meta.sizeBytes <= FULL_FETCH_CAP) {
        stage.value = `${s.label}：对照组 —— 整体拉取全部字节到 JS`
        try {
          const full = await measureFullFetch(meta.id, meta.sizeBytes)
          row.fullFetchMs = full.ms
          row.fullFetchBytes = full.bytes
        } catch (e) {
          row.fullFetchMs = -1
          row.error = `整体拉取失败：${e instanceof Error ? e.message : String(e)}`
        }
      }

      // JSON 建索引。
      //
      // **按产品自己的分级规则来**：02 文档 §4.4 规定 >50MB 是 SummaryOnly 分级，
      // 树视图不可用，所以这里也不建索引。
      // 第一版量测台无视分级、对 100MB 也强行建索引，结果 Rust 进程冲到 1.4GB
      // 并长时间无响应 —— 那是量测台的错，不是产品的错。但这个数字本身是重要信息，
      // 所以记录下「为什么跳过」和「预估代价」，而不是默默不测。
      const memBefore = await ipc.processMemory().catch(() => null)
      row.memBeforeMb = memBefore?.available ? memBefore.privateBytes / 1048576 : null

      if (meta.tier === 'summaryOnly') {
        row.indexSkipped =
          `按 02 文档 §4.4 分级跳过：${meta.tier} 分级下树视图不可用，不建索引。` +
          `实测过强行建索引的代价：100MB body 会让 Rust 进程私有内存冲到约 1.4GB` +
          `（serde_json::Value 树约 14 倍膨胀），且长时间无响应。`
      } else {
        stage.value = `${s.label}：Rust 侧建 JSON 索引`
        try {
          const t = performance.now()
          const indexed = await ipc.indexResponse(meta.id)
          row.indexMs = performance.now() - t
          row.totalNodes = indexed.totalNodes ?? null
          row.indexBytes = indexed.indexBytes ?? null
          row.indexMetadataRatio = row.indexBytes === null
            ? null
            : row.indexBytes / meta.sizeBytes

          const t2 = performance.now()
          await ipc.getResponseTree(meta.id, '', 0, 200)
          row.treePageMs = performance.now() - t2

          const memAfter = await ipc.processMemory().catch(() => null)
          row.memAfterMb = memAfter?.available ? memAfter.privateBytes / 1048576 : null
          if (row.memBeforeMb !== null && row.memAfterMb !== null) {
            row.indexMemCostMb = row.memAfterMb - row.memBeforeMb
            row.indexMemRatio = row.indexMemCostMb / (meta.sizeBytes / 1048576)
          }
        } catch (e) {
          row.error =
            (row.error ? row.error + '；' : '') +
            `建索引失败：${e instanceof Error ? e.message : String(e)}`
        }
      }

      // 验收判定
      if (s.bytes === 10 * 1024 * 1024) {
        row.verdict = row.firstPaintMs < 300 ? 'PASS（10MB 首屏 < 300ms）' : `FAIL（首屏 ${row.firstPaintMs.toFixed(0)}ms ≥ 300ms）`
      } else if (s.bytes === 100 * 1024 * 1024) {
        row.verdict = 'PASS（100MB 未崩溃）'
      } else {
        row.verdict = '参考'
      }
    } catch (e) {
      row.error = e instanceof Error ? e.message : String(e)
      row.verdict = 'FAIL'
      probe.value = null
    }

    rows.value = [...rows.value, row]
  }

  stage.value = '写报告'
  const r10 = rows.value.find((r) => r.targetBytes === 10 * 1024 * 1024)
  const r100 = rows.value.find((r) => r.targetBytes === 100 * 1024 * 1024)
  const memNow = await ipc.processMemory().catch(() => null)

  const report = {
    spike: 'S2',
    at: new Date().toISOString(),
    inTauri,
    userAgent: navigator.userAgent,
    firstChunkBytes: FIRST_CHUNK,
    fullFetchCapBytes: FULL_FETCH_CAP,
    acceptance: {
      '10MB 首屏 < 300ms': {
        measuredMs: r10?.firstPaintMs ?? null,
        pass: r10 != null && !r10.error && r10.firstPaintMs > 0 && r10.firstPaintMs < 300,
      },
      '100MB 不崩溃且可预览': {
        // 「不崩溃」= 跑完了这一行、拿到了首屏、进程还活着能继续应答 IPC
        rendered: r100 != null && !r100.error && r100.firstPaintMs > 0,
        firstPaintMs: r100?.firstPaintMs ?? null,
        pass: r100 != null && !r100.error && r100.firstPaintMs > 0,
      },
    },
    /** 全部跑完后的 Rust 进程内存，用于确认没有累积泄漏 */
    memAfterAllMb: memNow?.available ? memNow.privateBytes / 1048576 : null,
    rows: rows.value,
  }
  try {
    savedTo.value = await ipc.spikeReport('s2', JSON.stringify(report, null, 2))
  } catch (e) {
    savedTo.value = `写盘失败：${e instanceof Error ? e.message : String(e)}`
  }

  stage.value = ''
  running.value = false
  done.value = true
  ;(window as unknown as { __s2Done: boolean }).__s2Done = true
}

function fmt(n: number | null, unit = 'ms') {
  return n === null ? '—' : n < 0 ? '失败' : `${n.toFixed(1)}${unit}`
}
function mb(n: number | null) {
  return n === null ? '—' : `${(n / 1048576).toFixed(2)} MB`
}

// 支持无人值守运行：以 VITE_AUTORUN_S2=1 构建/启动时自动跑并落盘，
// 这样不需要有人点按钮也能拿到结果。
if (import.meta.env.VITE_AUTORUN_S2 === '1') {
  setTimeout(() => void runAll(), 300)
}

defineExpose({ runAll })
</script>

<template>
  <div class="s2">
    <header>
      <h2>Spike S2 · 自定义协议 + Range → CodeMirror</h2>
      <p class="sub">
        验收：10MB 首屏 &lt; 300ms，100MB 不崩溃。
        环境：{{ inTauri ? 'Tauri WebView' : '普通浏览器（无自定义协议，仅供对照）' }}
      </p>
      <div class="bar">
        <button type="button" :disabled="running" @click="runAll()">
          {{ running ? '运行中…' : '开始量测' }}
        </button>
        <span v-if="stage" class="stage">{{ stage }}</span>
        <span v-if="savedTo" class="saved">报告：{{ savedTo }}</span>
      </div>
    </header>

    <table v-if="rows.length" class="grid">
      <thead>
        <tr>
          <th>体积</th><th>实际</th><th>分级</th><th>高亮</th>
          <th>Rust生成</th>
          <th>Range取64KB</th><th>建编辑器</th><th class="hi">首帧</th><th>DOM</th>
          <th>对照:全量拉取</th>
          <th>建索引</th><th>节点数</th><th>索引元数据</th><th>树首页</th>
          <th>判定</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in rows" :key="r.size">
          <td>{{ r.size }}</td>
          <td>{{ mb(r.actualBytes) }}</td>
          <td>{{ r.tier }}</td>
          <td>{{ r.highlight ? '开' : '关' }}</td>
          <td>{{ fmt(r.genMs) }}</td>
          <td>{{ fmt(r.rangeFetchMs) }}</td>
          <td>{{ fmt(r.editorMs) }}</td>
          <td class="hi">{{ fmt(r.firstPaintMs) }}</td>
          <td>{{ r.domNodes }}</td>
          <td>{{ fmt(r.fullFetchMs) }}</td>
          <td>{{ fmt(r.indexMs) }}</td>
          <td>{{ r.totalNodes ?? '—' }}</td>
          <td>{{ r.indexBytes === null ? '—' : `${mb(r.indexBytes)} / ${r.indexMetadataRatio?.toFixed(2)}×` }}</td>
          <td>{{ fmt(r.treePageMs) }}</td>
          <td :class="{ pass: r.verdict.startsWith('PASS'), fail: r.verdict.startsWith('FAIL') }">
            {{ r.verdict }}
            <div v-if="r.error" class="err">{{ r.error }}</div>
          </td>
        </tr>
      </tbody>
    </table>

    <!-- 量测位：CodeMirrorView 在这里被反复挂载/卸载 -->
    <div class="probe">
      <p class="label">量测位</p>
      <CodeMirrorView
        v-if="probe"
        :key="probe.id"
        :response-id="probe.id"
        :total-bytes="probe.bytes"
        :highlight="probe.highlight"
        :first-chunk-bytes="FIRST_CHUNK"
        @metrics="onMetrics"
        @failed="onFailed"
      />
      <p v-else class="idle">（空闲）</p>
    </div>
  </div>
</template>

<style scoped>
.s2 { display: flex; flex-direction: column; height: 100%; min-height: 0; overflow: auto; padding: 12px; gap: 12px; }
h2 { margin: 0 0 4px; font-size: var(--a-fs-lg); }
.sub { margin: 0 0 8px; font-size: var(--a-fs-sm); color: var(--a-text-dim); }
.bar { display: flex; align-items: center; gap: 10px; }
.bar button {
  height: var(--a-h-control-sm); padding: 0 14px; background: var(--a-primary);
  border: none; border-radius: var(--a-radius-md); color: var(--a-on-primary);
  font: inherit; cursor: pointer;
}
.bar button:disabled { opacity: 0.5; cursor: default; }
.stage { font-size: var(--a-fs-sm); color: var(--a-warn); }
.saved { font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); color: var(--a-text-faint); }
.grid { border-collapse: collapse; font: var(--a-fs-xs)/1.6 var(--a-font-mono); }
.grid th, .grid td {
  border: 1px solid var(--a-border); padding: 3px 6px; text-align: right; white-space: nowrap;
}
.grid th { background: var(--a-bg-2); color: var(--a-text-dim); font-weight: 500; }
.grid td:first-child, .grid th:first-child { text-align: left; }
.grid .hi { background: var(--a-selected); font-weight: var(--a-fw-semibold); }
.pass { color: var(--a-ok); }
.fail { color: var(--a-error); }
.err { color: var(--a-error); font-size: var(--a-fs-2xs); white-space: normal; max-width: 240px; }
.probe {
  border: 1px dashed var(--a-border-strong); border-radius: var(--a-radius-md);
  min-height: 220px; display: flex; flex-direction: column;
}
.label { margin: 0; padding: 4px 8px; font-size: var(--a-fs-xs); color: var(--a-text-faint); border-bottom: 1px dashed var(--a-border); }
.idle { padding: 12px; color: var(--a-text-faint); font-size: var(--a-fs-sm); }
</style>
