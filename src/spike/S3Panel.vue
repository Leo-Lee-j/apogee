<script setup lang="ts">
import { ref, shallowRef, nextTick, onBeforeUnmount } from 'vue'
import { ipc, inTauri } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import Sidebar from '@/components/sidebar/Sidebar.vue'
import S3FullTree from './S3FullTree.vue'
import { OVERSCAN, sidebarRowHeightForUiSize, synthTree, visibleNodes, type FlatNode } from '@/components/sidebar/tree'

/**
 * # Spike S3 量测台：自研虚拟树的帧率与首渲
 *
 * ## 要验证的（03 文档 §10）
 *
 * > S3：自研虚拟树渲染 5000 节点。成功标准：滚动 60fps，首次渲染 < 100ms。
 *
 * 顺带验 PRD §5.1 的「集合树渲染 < 100ms（1000 个请求的集合首次展开）」。
 *
 * ## 为什么 S3 还要管内存
 *
 * S1 发现空闲内存 146.7MB 里单个 `gpu-process` 就占 67MB（46%）。关掉硬件加速
 * 能省下这部分，代价是滚动交给 CPU 合成 —— 而滚动帧率正是本 Spike 的主题。
 * 所以 03 文档 §10.4 把「内存门槛三选一」挂在 S3 后面：**先知道关 GPU 掉多少帧，
 * 再决定要不要用帧率换内存。** 本面板负责帧率那一半，
 * `scripts/measure-s3-windows.ps1` 负责在 GPU 开/关两种配置下各跑一遍并量进程树内存。
 *
 * ## 三条量测口径（口径不写清等于没测）
 *
 * 1. **帧率不按「fps ≥ 60」判定，按掉帧率判定。** 在 60Hz 显示器上 60fps 就是物理
 *    上限，采样噪声就能让一个达标的实现报出 59.4 而「不达标」。所以先测本机
 *    显示器的实际 rAF 节奏作为基准，再按「相对该节奏掉了多少帧」判定。
 *    这也是 Chromium 自己的 smoothness 口径。
 * 2. **滚动用主线程驱动**（每帧改 `scrollTop`），不是合成器驱动。真实滚轮滚动在
 *    Chromium 里可以走合成器线程，主线程驱动是**更悲观**的一侧，且它才会每帧
 *    触发我们的 slice 重算 + 行 DOM 更新 —— 那正是要测的东西。代价是这个数字
 *    不包含真实输入延迟，见「未量测」。
 * 3. **首渲拆成三段**：数据准备（正式实现里由 Rust 出，不该算进前端）、
 *    `visible` 过滤（O(N)）、DOM 首帧（O(视口)）。只报一个总数看不出 5000 节点的
 *    成本落在哪，也就无法判断哪一段会随规模爆掉。
 */

// ── 量测参数 ──────────────────────────────────────────────────────────

/**
 * 三个规模档位。节点数按「文件夹 + 请求」凑，与真实集合结构一致。
 *
 * 全部**默认展开**：折叠的文件夹下的子节点根本不进 `visible`，
 * 「合成了 5000 个节点」和「虚拟滚动要处理 5000 行」是两件事，
 * 不展开就等于没压到虚拟滚动。
 */
const SCALES = [
  { key: 'prd-1000', label: '1000 请求', folders: 40, perFolder: 25, gate: 'PRD §5.1 集合树 < 100ms' },
  { key: 's3-5000', label: '5000 节点', folders: 100, perFolder: 49, gate: '03 §10.1 S3 首渲 < 100ms + 60fps' },
  { key: 'stress-20000', label: '20000 节点', folders: 200, perFolder: 99, gate: '余量观测（无门槛）' },
] as const

/** 滚动采样帧数。300 帧在 60Hz 下约 5 秒，足够让 p95/p99 有意义又不至于把量测拖长。 */
const SCROLL_FRAMES = 300

/**
 * 滚动采样的墙钟上限。
 *
 * 对照组（全量渲染 5000 行）单帧可能到几百毫秒，300 帧就是几十秒，会把量测拖成
 * 无法在 CI 里跑的东西。到时就停，并把**实际采到的帧数**记进报告 ——
 * 帧时间分布不受截断影响，但「采了多少帧」必须诚实报出来。
 */
const SCROLL_MAX_MS = 10_000

/**
 * 每帧滚动 8 行（176px）—— 一次快速 fling 的量级。
 *
 * 固定「行/帧」而不是「总高度 / 帧数」，是为了让三个档位的**每帧工作量相同**：
 * 这样如果帧时间随节点数上升，就一定是虚拟化本身出了问题，而不是因为高档位
 * 被迫滚得更快。这条是本 Spike 想验证的核心架构论点。
 */
const SCROLL_STEP_ROWS = 8

/** 对照组（全量渲染）只跑前两档。20000 行 DOM 会把量测台自己拖死，跳过并在报告里说明。 */
const FULL_RENDER_MAX_NODES = 5001

/** 展开/折叠重复次数，取中位数。单次会被一次 GC 或一次调度抖动带偏。 */
const TOGGLE_REPEATS = 7

/**
 * `visible` 过滤的重复次数。
 *
 * `performance.now()` 在 Chromium 里被钳到 0.1ms 粒度，1000 节点跑一趟只要几十微秒，
 * 单次测量只会读出 0.00 或 0.10 —— 那不是数据，是量化噪声。重复 200 次再平均，
 * 把总耗时推到毫秒量级之上，档位之间的差异才是真的。
 */
const VISIBLE_FILTER_REPEATS = 200

/**
 * 内存驻留阶段时长。量测台在这个窗口内采进程树私有工作集。
 *
 * 定 30s 而不是「够用就好」：量测台采一次样要跑两条 CIM 查询
 * （`Win32_Process` + `Win32_PerfRawData_PerfProc_Process`），耗时随机器上的进程总数变化，
 * 慢机器上一次能到一两秒。窗口给太紧，量测就会因为采样跑出窗口而失败 ——
 * 而这属于「量测台自己的调度问题」，不该表现为被测对象的失败。
 * 窗口截止时刻会通过阶段文件告知量测台，由它自己决定采几次。
 */
const MEMORY_HOLD_MS = 30_000

// ── 类型 ──────────────────────────────────────────────────────────────

interface FrameStats {
  frames: number
  /** 是否因墙钟上限提前收尾（对照组的慢帧会触发） */
  truncated: boolean
  durationMs: number
  fps: number
  p50Ms: number
  p95Ms: number
  p99Ms: number
  maxMs: number
  /** 相对显示器实际节奏推算的丢帧数 */
  droppedFrames: number
  droppedPct: number
  /** 超过一帧预算 1.5 倍的帧数 */
  longFrames: number
}

interface LoafStats {
  supported: boolean
  /** 超过 50ms 的动画帧数量（LoAF 只上报这一类）。0 是好结果。 */
  count: number
  maxDurationMs: number | null
  maxBlockingMs: number | null
  /** 最长那一帧里 style+layout 占了多少 —— 区分「JS 慢」和「布局慢」 */
  maxStyleAndLayoutMs: number | null
}

interface StepCost {
  medianMs: number
  p95Ms: number
  maxMs: number
  iterations: number
}

interface Cell {
  scaleKey: string
  scale: string
  impl: 'virtual' | 'full'
  gate: string
  totalNodes: number
  visibleNodes: number
  renderedRows: number
  /** 实际传给被测组件的动态行高，必须与 Sidebar 的 CSS 行高一致。 */
  rowHeightPx: number
  /** 合成扁平数组的耗时。正式实现里这段在 Rust 侧，单列出来避免算进前端预算。 */
  dataPrepMs: number
  /** O(N) 的 visible 过滤，重复 VISIBLE_FILTER_REPEATS 次取平均（单次低于计时器精度） */
  visibleFilterMs: number
  /**
   * 数据就绪 → Vue 打完 DOM（纯 JS + DOM 变更，**不含 vsync 等待**）。
   * 这是「我们的代码有多快」的真数字；下面两个含显示器等待。
   */
  patchMs: number
  /** 强制同步 style + layout 的耗时（读 offsetHeight 触发） */
  layoutMs: number
  /** 数据就绪 → 本帧 rAF 回调（含最多 1 个 vsync 等待） */
  commitMs: number
  /** 数据就绪 → 像素确实上屏（含最多 2 个 vsync 等待）。用户可感知延迟。 */
  firstPaintMs: number
  scrollHeightPx: number
  scroll: FrameStats | null
  /** 一次滚动步进的主线程成本（patch + 强制 layout，不含 paint/composite） */
  stepCost: StepCost | null
  loaf: LoafStats | null
  /** 展开/折叠：patch + layout，不含 vsync 等待。这是 O(N) 那趟扫描的真成本。 */
  togglePatchMedianMs: number | null
  togglePatchMaxMs: number | null
  /** 展开/折叠到像素上屏的耗时（含 vsync 等待），用户可感知延迟。对照组不测。 */
  toggleMedianMs: number | null
  toggleMaxMs: number | null
  error: string | null
}

// ── 状态 ──────────────────────────────────────────────────────────────

const cells = ref<Cell[]>([])
const running = ref(false)
const stage = ref('')
const done = ref(false)
const savedTo = ref<string | null>(null)
const holdRemaining = ref(0)
const display = ref<{
  frameBudgetMs: number
  hz: number
  nominalHz: number
  sampleCount: number
  minMs: number
  maxMs: number
} | null>(null)

/** 挂在量测位上的被测组件。key 变化即强制重建，保证每次都是「首次渲染」。 */
const probe = shallowRef<{ impl: 'virtual' | 'full'; nodes: FlatNode[]; key: string } | null>(null)
const probeRef = ref<{
  scroller: HTMLElement | null
  stats: () => {
    total: number
    visible: number
    rendered: number
    rowHeight: number
    overscan: number
    scrollHeight: number
  }
  scrollTo: (px: number) => void
  toggleFolderAt: (index: number) => void
} | null>(null)

const tabs = useTabsStore()
const ui = useUiStore()

// ── 通用工具 ──────────────────────────────────────────────────────────

function nextFrame(): Promise<number> {
  return new Promise((resolve) => requestAnimationFrame(resolve))
}

/**
 * 等到「上一帧的像素确实上屏」。
 *
 * rAF 回调在**本帧绘制之前**执行，所以只等一次 rAF 只能证明「工作提交了」，
 * 不能证明「画出来了」。等第二次 rAF 时浏览器已经进入下一帧，上一帧必然已绘制。
 * 代价是这个口径含最多一个 vsync 的等待（60Hz 下约 16.7ms），
 * 所以 `commitMs` 和 `firstPaintMs` 两个都报，不合成一个数。
 */
async function twoFrames(): Promise<void> {
  await nextFrame()
  await nextFrame()
}

function median(values: number[]): number {
  if (values.length === 0) return 0
  const s = [...values].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  return s.length % 2 === 1 ? s[mid]! : (s[mid - 1]! + s[mid]!) / 2
}

function percentile(values: number[], p: number): number {
  if (values.length === 0) return 0
  const s = [...values].sort((a, b) => a - b)
  return s[Math.max(0, Math.ceil(p * s.length) - 1)]!
}

function deltas(timestamps: number[]): number[] {
  const out: number[] = []
  for (let i = 1; i < timestamps.length; i++) out.push(timestamps[i]! - timestamps[i - 1]!)
  return out
}

// ── 显示器基线 ────────────────────────────────────────────────────────

/**
 * 测本机显示器的实际 rAF 节奏。
 *
 * 没有这一步就无法解读帧率：60Hz 显示器上 60fps 是天花板（此时「60fps」等价于
 * 「一帧不掉」），而 120Hz 显示器上报出 60fps 说明我们每两帧丢一帧。
 * 同一个 fps 数字在两种显示器上含义相反。
 */
async function measureDisplayBaseline(frames = 120) {
  const ts: number[] = []
  await new Promise<void>((resolve) => {
    const tick = (t: number) => {
      ts.push(t)
      if (ts.length >= frames) resolve()
      else requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  })
  const d = deltas(ts)
  const budget = median(d)
  const hz = 1000 / budget
  // 常见刷新率里取最近的一个作为「标称值」，只用于报告可读性，判定一律用实测 budget
  const nominal = [60, 75, 90, 100, 120, 144, 165, 240].reduce((best, cand) =>
    Math.abs(cand - hz) < Math.abs(best - hz) ? cand : best,
  )
  return {
    frameBudgetMs: budget,
    hz,
    nominalHz: nominal,
    sampleCount: d.length,
    minMs: Math.min(...d),
    maxMs: Math.max(...d),
  }
}

// ── LoAF ──────────────────────────────────────────────────────────────

interface LoafEntryLike extends PerformanceEntry {
  renderStart?: number
  styleAndLayoutStart?: number
  blockingDuration?: number
}

const loafSupported =
  typeof PerformanceObserver !== 'undefined' &&
  (PerformanceObserver.supportedEntryTypes ?? []).includes('long-animation-frame')

let loafObserver: PerformanceObserver | null = null
let loafEntries: LoafEntryLike[] = []

function startLoaf() {
  loafEntries = []
  if (!loafSupported) return
  loafObserver = new PerformanceObserver((list) => {
    loafEntries.push(...(list.getEntries() as LoafEntryLike[]))
  })
  loafObserver.observe({ type: 'long-animation-frame', buffered: false })
}

function stopLoaf(): LoafStats {
  loafObserver?.disconnect()
  loafObserver = null
  if (!loafSupported) {
    // 拿不到就报「不支持」，绝不报 0 —— 那会被读成「一帧都没超时」
    return { supported: false, count: 0, maxDurationMs: null, maxBlockingMs: null, maxStyleAndLayoutMs: null }
  }
  const entries = loafEntries
  loafEntries = []
  if (entries.length === 0) {
    return { supported: true, count: 0, maxDurationMs: null, maxBlockingMs: null, maxStyleAndLayoutMs: null }
  }
  const worst = entries.reduce((a, b) => (b.duration > a.duration ? b : a))
  const styleAndLayout =
    worst.styleAndLayoutStart != null && worst.styleAndLayoutStart > 0
      ? worst.startTime + worst.duration - worst.styleAndLayoutStart
      : null
  return {
    supported: true,
    count: entries.length,
    maxDurationMs: worst.duration,
    maxBlockingMs: Math.max(...entries.map((e) => e.blockingDuration ?? 0)),
    maxStyleAndLayoutMs: styleAndLayout,
  }
}

// ── 滚动量测 ──────────────────────────────────────────────────────────

/**
 * 主线程驱动滚动，逐帧采 rAF 时间戳。
 *
 * 到边界就折返（ping-pong），保证 300 帧里每一帧都真的产生了新的可视区间 ——
 * 卡在底部空转的帧是「什么都没渲染」的帧，会把帧率测得虚高。
 */
async function measureScroll(el: HTMLElement, stepPx: number, budgetMs: number): Promise<FrameStats> {
  el.scrollTop = 0
  await twoFrames()

  const limit = Math.max(0, el.scrollHeight - el.clientHeight)
  const ts: number[] = []
  let pos = 0
  let dir = 1
  const wallStart = performance.now()
  let truncated = false

  await new Promise<void>((resolve) => {
    const tick = (t: number) => {
      ts.push(t)
      if (ts.length > SCROLL_FRAMES) {
        resolve()
        return
      }
      if (performance.now() - wallStart > SCROLL_MAX_MS) {
        truncated = true
        resolve()
        return
      }
      pos += dir * stepPx
      if (pos >= limit) {
        pos = limit
        dir = -1
      } else if (pos <= 0) {
        pos = 0
        dir = 1
      }
      el.scrollTop = pos
      requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  })

  const d = deltas(ts)
  const durationMs = ts[ts.length - 1]! - ts[0]!
  const expectedFrames = durationMs / budgetMs
  const droppedFrames = d.reduce((acc, x) => acc + Math.max(0, Math.round(x / budgetMs) - 1), 0)
  return {
    frames: ts.length,
    truncated,
    durationMs,
    fps: (d.length / durationMs) * 1000,
    p50Ms: median(d),
    p95Ms: percentile(d, 0.95),
    p99Ms: percentile(d, 0.99),
    maxMs: Math.max(...d),
    droppedFrames,
    droppedPct: expectedFrames > 0 ? (droppedFrames / expectedFrames) * 100 : 0,
    longFrames: d.filter((x) => x > budgetMs * 1.5).length,
  }
}

/**
 * 一次滚动步进的**主线程成本**。
 *
 * 为什么需要这个：帧间隔在这台机器上是饱和的 —— 只要单帧工作量低于显示器节奏
 * （13.3ms），所有实现都会报出一模一样的 75fps 和 0% 掉帧。那能证明「不掉帧」，
 * 但证明不了「还剩多少余量」。所以另外测一次不受 vsync 影响的单步成本：
 * 直接调 `scrollTo` 同步改状态 → 等 Vue 打完 DOM → 读 `offsetHeight` 强制 style+layout。
 *
 * 不含 paint / composite（那部分在合成器线程，也是 GPU 开关真正影响的地方），
 * 所以这个数是「主线程侧成本」，不是完整帧成本。
 */
async function measureStepCost(
  inst: { scrollTo: (px: number) => void },
  el: HTMLElement,
  stepPx: number,
  iterations = 60,
): Promise<StepCost> {
  const limit = Math.max(1, el.scrollHeight - el.clientHeight)
  const samples: number[] = []
  let pos = 0
  for (let i = 0; i < iterations; i++) {
    pos = (pos + stepPx) % limit
    const t0 = performance.now()
    inst.scrollTo(pos)
    await nextTick()
    void el.offsetHeight
    samples.push(performance.now() - t0)
  }
  return {
    medianMs: median(samples),
    p95Ms: percentile(samples, 0.95),
    maxMs: Math.max(...samples),
    iterations,
  }
}

/**
 * 展开/折叠的耗时，两个口径都测。
 *
 * 这条路径是 O(N)：`all` 换引用 → `visible` 全量重算。滚动是 O(视口)，所以两者
 * 必须分开测 —— 只测滚动会漏掉「20000 节点下点一下文件夹掉一帧」这类问题。
 *
 * `patch` 是不含 vsync 等待的真实计算成本，`paint` 是用户感知到的延迟。
 * 只报后者会得出「1000 和 20000 节点一样快」的错误结论 —— 那只是因为两者
 * 都远低于一帧，被显示器节奏拉平了。
 */
async function measureToggle(
  inst: { toggleFolderAt: (index: number) => void },
  el: HTMLElement,
): Promise<{ patchMedianMs: number; patchMaxMs: number; paintMedianMs: number; paintMaxMs: number }> {
  const patch: number[] = []
  const paint: number[] = []
  for (let i = 0; i < TOGGLE_REPEATS; i++) {
    const t0 = performance.now()
    inst.toggleFolderAt(0)
    await nextTick()
    void el.offsetHeight
    patch.push(performance.now() - t0)
    await twoFrames()
    paint.push(performance.now() - t0)
  }
  return {
    patchMedianMs: median(patch),
    patchMaxMs: Math.max(...patch),
    paintMedianMs: median(paint),
    paintMaxMs: Math.max(...paint),
  }
}

// ── 单格量测 ──────────────────────────────────────────────────────────

async function runCell(
  scale: (typeof SCALES)[number],
  impl: 'virtual' | 'full',
  budgetMs: number,
): Promise<Cell> {
  const cell: Cell = {
    scaleKey: scale.key,
    scale: scale.label,
    impl,
    gate: impl === 'virtual' ? scale.gate : '对照组（无门槛）',
    totalNodes: 0,
    visibleNodes: 0,
    renderedRows: 0,
    rowHeightPx: 0,
    dataPrepMs: 0,
    visibleFilterMs: 0,
    patchMs: 0,
    layoutMs: 0,
    commitMs: 0,
    firstPaintMs: 0,
    scrollHeightPx: 0,
    scroll: null,
    stepCost: null,
    loaf: null,
    togglePatchMedianMs: null,
    togglePatchMaxMs: null,
    toggleMedianMs: null,
    toggleMaxMs: null,
    error: null,
  }

  try {
    // ① 数据准备。正式实现里这段由 Rust 侧 list_tree 出，不该算进前端首渲预算，
    //    但也不能不报 —— 不报就等于把一段真实成本藏起来了。
    const tData = performance.now()
    const nodes = synthTree({ folders: scale.folders, perFolder: scale.perFolder, expanded: 'all' })
    cell.dataPrepMs = performance.now() - tData
    cell.totalNodes = nodes.length

    // ② 单独测 O(N) 的 visible 过滤。它在首渲和每次展开/折叠时都会跑一遍。
    //
    //    必须重复多次再平均：`performance.now()` 在 Chromium 里被钳到 0.1ms 粒度，
    //    而 1000 节点跑一趟只要几十微秒 —— 单次测量会读出 0.00ms 或 0.10ms，
    //    然后「20 倍节点数只慢 1 倍」这种噪声就会被当成结论写进报告。
    const tFilter = performance.now()
    let vis: FlatNode[] = []
    for (let i = 0; i < VISIBLE_FILTER_REPEATS; i++) vis = visibleNodes(nodes)
    cell.visibleFilterMs = (performance.now() - tFilter) / VISIBLE_FILTER_REPEATS
    cell.visibleNodes = vis.length

    // ③ 卸载上一个被测组件，等它彻底离开 DOM，否则上一次的 20000 行还在
    //    影响本次的布局耗时。
    probe.value = null
    await nextTick()
    await twoFrames()

    // ④ 首渲。四个口径分开记，因为它们回答的是不同问题：
    //    patch = 我们的代码有多快（无 vsync 等待）
    //    layout = 浏览器排版这批 DOM 要多久（强制同步触发）
    //    commit / firstPaint = 用户等了多久（含 1 / 2 个 vsync）
    const t0 = performance.now()
    probe.value = { impl, nodes, key: `${scale.key}-${impl}-${Date.now()}` }
    await nextTick()
    cell.patchMs = performance.now() - t0

    const inst = probeRef.value
    if (!inst) throw new Error('被测组件未挂载成功，拿不到实例')
    const el = inst.scroller
    if (!el) throw new Error('被测组件没有暴露滚动容器')

    const tLayout = performance.now()
    void el.offsetHeight
    cell.layoutMs = performance.now() - tLayout

    await nextFrame()
    cell.commitMs = performance.now() - t0
    await nextFrame()
    cell.firstPaintMs = performance.now() - t0

    const stats = inst.stats()
    cell.renderedRows = stats.rendered
    cell.rowHeightPx = stats.rowHeight
    cell.scrollHeightPx = stats.scrollHeight

    // ⑤ 滚动帧率 + 同窗口的 LoAF
    startLoaf()
    cell.scroll = await measureScroll(el, SCROLL_STEP_ROWS * stats.rowHeight, budgetMs)
    cell.loaf = stopLoaf()

    // ⑥ 单步滚动的主线程成本。帧间隔饱和时这是唯一能看出余量的数字。
    cell.stepCost = await measureStepCost(inst, el, SCROLL_STEP_ROWS * stats.rowHeight)

    // ⑦ 展开/折叠（只测产品实现；对照组没有这条路径）
    if (impl === 'virtual') {
      const toggle = await measureToggle(inst, el)
      cell.togglePatchMedianMs = toggle.patchMedianMs
      cell.togglePatchMaxMs = toggle.patchMaxMs
      cell.toggleMedianMs = toggle.paintMedianMs
      cell.toggleMaxMs = toggle.paintMaxMs
    }
  } catch (e) {
    cell.error = e instanceof Error ? `${e.name}: ${e.message}` : String(e)
  }

  return cell
}

// ── 主流程 ────────────────────────────────────────────────────────────

/** 把阶段写到 `s3-phase.json`，PS1 靠它决定何时采内存。拿不到 Tauri 时静默跳过。 */
async function reportPhase(phase: string, extra: Record<string, unknown> = {}) {
  if (!inTauri) return
  try {
    await ipc.spikeReport(
      's3-phase',
      JSON.stringify({ phase, at: new Date().toISOString(), ...extra }, null, 2),
    )
  } catch {
    /* 阶段文件写不进去不该让量测失败，PS1 会因超时报错 */
  }
}

let holdTimer: ReturnType<typeof setInterval> | null = null

/**
 * 运行模式。
 *
 * `full` 跑帧率矩阵再驻留；`memoryOnly` 直接进驻留，中间什么都不跑。
 *
 * 为什么要分开：帧率矩阵会创建/销毁 5 次组件，其中两次铺了 1040 和 5000 行真实 DOM，
 * 还合成了三份最多 20000 个对象的数组。跑完立刻量内存，量到的是「一次重负载会话之后」
 * 的残留（分离 DOM 尚未回收、堆已经涨上去），不是「3 标签 + 5000 节点树」的稳态。
 * 而内存门槛要拿后者去定。所以量测台会为每种 GPU 配置各起两次进程：一次测帧率，
 * 一次干净驻留量内存。两个数都报 —— 差值本身就是「重负载会话之后内存回不回来」的答案。
 */
type RunMode = 'full' | 'memoryOnly'
const mode = ref<RunMode>('full')

/** 进入内存驻留状态：5000 节点集合树 + 3 个标签，然后空转。 */
async function enterHoldState() {
  probe.value = null
  await nextTick()
  const holdNodes = synthTree({ folders: 100, perFolder: 49, expanded: 'all' })
  probe.value = { impl: 'virtual', nodes: holdNodes, key: `hold-${Date.now()}` }
  while (tabs.tabs.length < 3) {
    tabs.open({ title: `请求 ${tabs.tabs.length + 1}`, method: 'GET', url: 'https://example.invalid/api' })
  }
  await nextTick()
  await twoFrames()
  return holdNodes.length
}

async function runAll(runMode: RunMode = mode.value) {
  if (running.value) return
  running.value = true
  mode.value = runMode
  done.value = false
  cells.value = []
  savedTo.value = null

  await reportPhase('starting', { mode: runMode })

  const memBefore = await ipc.processMemory().catch(() => null)
  let budgetMs = 1000 / 60

  if (runMode === 'full') {
    // ① 显示器基线。必须在任何渲染压力之前测，否则测到的是「我们正忙」而不是「屏幕多快」。
    stage.value = '测显示器基线'
    display.value = await measureDisplayBaseline()
    budgetMs = display.value.frameBudgetMs

    // ② 帧率矩阵。这一段**绝不能**有外部采样干扰，所以 PS1 在 phase=fps 期间不碰进程树。
    await reportPhase('fps', { frameBudgetMs: budgetMs, nominalHz: display.value.nominalHz })
    for (const scale of SCALES) {
      stage.value = `虚拟滚动 · ${scale.label}`
      cells.value = [...cells.value, await runCell(scale, 'virtual', budgetMs)]
    }
    for (const scale of SCALES) {
      if (scale.folders * (scale.perFolder + 1) >= FULL_RENDER_MAX_NODES) continue
      stage.value = `对照·全量渲染 · ${scale.label}`
      cells.value = [...cells.value, await runCell(scale, 'full', budgetMs)]
    }
  }

  // ③ 内存驻留。PRD §5.1 声明的量测条件是「1 工作区、3 标签」，这里还多挂一棵
  //    5000 节点的集合树 —— 比 PRD 的条件更重，所以数字是上界。
  stage.value = '内存驻留（等量测台采样）'
  const treeNodes = await enterHoldState()

  const memAtHold = await ipc.processMemory().catch(() => null)
  await reportPhase('memory-hold', {
    mode: runMode,
    holdMs: MEMORY_HOLD_MS,
    /**
     * 驻留窗口的截止时刻，**epoch 毫秒数**。
     *
     * 刻意给数字而不是让量测台去解析 `at` 那个 ISO 字符串：PowerShell 的
     * `ConvertFrom-Json` 会把 ISO 字符串自动转成 `DateTime`，再 `[string]` 一次就把时区丢了，
     * UTC 值被当成本地时间重新解析，结果偏掉整个时区偏移量。
     * S1 的就绪标记用 `readyEpochMs` 而不是时间字符串，正是同一个理由。
     */
    holdUntilEpochMs: Date.now() + MEMORY_HOLD_MS,
    condition:
      runMode === 'full'
        ? '1 工作区 + 3 标签 + 5000 节点集合树（跑完帧率矩阵之后，含会话残留）'
        : '1 工作区 + 3 标签 + 5000 节点集合树（干净启动，未跑帧率矩阵）',
    treeNodes,
  })

  holdRemaining.value = Math.round(MEMORY_HOLD_MS / 1000)
  holdTimer = setInterval(() => {
    holdRemaining.value = Math.max(0, holdRemaining.value - 1)
  }, 1000)
  await new Promise((resolve) => setTimeout(resolve, MEMORY_HOLD_MS))
  if (holdTimer) clearInterval(holdTimer)
  holdTimer = null

  const memAfter = await ipc.processMemory().catch(() => null)

  // ④ 落盘
  stage.value = '写报告'
  const report = buildReport(runMode, memBefore, memAtHold, memAfter)
  await reportPhase('done', { mode: runMode })
  if (inTauri) {
    try {
      savedTo.value = await ipc.spikeReport('s3-frontend', JSON.stringify(report, null, 2))
    } catch (e) {
      savedTo.value = `写盘失败：${e instanceof Error ? e.message : String(e)}`
    }
  } else {
    savedTo.value = '非 Tauri 环境，未落盘（仅供对照）'
  }

  stage.value = ''
  running.value = false
  done.value = true
  ;(window as unknown as { __s3Done: boolean }).__s3Done = true
}

function buildReport(
  runMode: RunMode,
  memBefore: { privateBytes: number; available: boolean } | null,
  memAtHold: { privateBytes: number; available: boolean } | null,
  memAfter: { privateBytes: number; available: boolean } | null,
) {
  const virtual5000 = cells.value.find((c) => c.scaleKey === 's3-5000' && c.impl === 'virtual')
  const virtual1000 = cells.value.find((c) => c.scaleKey === 'prd-1000' && c.impl === 'virtual')
  const budgetMs = display.value?.frameBudgetMs ?? 1000 / 60

  return {
    spike: 'S3',
    part: runMode === 'full' ? 'frontend' : 'memory-only',
    mode: runMode,
    at: new Date().toISOString(),
    inTauri,
    userAgent: navigator.userAgent,
    /** 被测对象是产品组件本身，不是量测台里的副本 */
    subject: {
      component: 'src/components/sidebar/Sidebar.vue',
      sharedModule: 'src/components/sidebar/tree.ts',
      rowHeightPx: virtual5000?.rowHeightPx || sidebarRowHeightForUiSize(ui.uiFontSize),
      overscanRows: OVERSCAN,
      control: 'src/spike/S3FullTree.vue（全量 v-for，不虚拟化）',
    },
    display: display.value,
    method: {
      scrollFrames: SCROLL_FRAMES,
      scrollStepRows: SCROLL_STEP_ROWS,
      scrollStepPx: SCROLL_STEP_ROWS * (virtual5000?.rowHeightPx || sidebarRowHeightForUiSize(ui.uiFontSize)),
      scrollDriver: '主线程：每帧写 scroller.scrollTop（比合成器驱动更悲观）',
      frameBudgetMs: budgetMs,
      frameBudgetSource: '本机显示器实测 rAF 中位间隔',
      toggleRepeats: TOGGLE_REPEATS,
      visibleFilterRepeats: VISIBLE_FILTER_REPEATS,
      visibleFilterNote:
        'performance.now() 在 Chromium 里钳到 0.1ms，单次跑不出有意义的数，所以重复取平均',
      fullRenderSkippedAbove: FULL_RENDER_MAX_NODES,
    },
    acceptance: {
      'S3 首渲 5000 节点 < 100ms': {
        measuredMs: virtual5000?.firstPaintMs ?? null,
        commitMs: virtual5000?.commitMs ?? null,
        pass: virtual5000 != null && !virtual5000.error && virtual5000.firstPaintMs < 100,
      },
      'S3 滚动 5000 节点 60fps': {
        fps: virtual5000?.scroll?.fps ?? null,
        droppedPct: virtual5000?.scroll?.droppedPct ?? null,
        p95Ms: virtual5000?.scroll?.p95Ms ?? null,
        frameBudgetMs: budgetMs,
        // 60Hz 显示器上 fps 上限就是 60，所以判定按掉帧率来，阈值 5%（Chromium smoothness 口径）
        criterion: '掉帧率 < 5%（相对本机显示器实测节奏）',
        pass:
          virtual5000?.scroll != null && !virtual5000.error && virtual5000.scroll.droppedPct < 5,
      },
      'PRD 集合树 1000 请求 < 100ms': {
        measuredMs: virtual1000?.firstPaintMs ?? null,
        requestCount: virtual1000 ? virtual1000.totalNodes - 40 : null,
        pass: virtual1000 != null && !virtual1000.error && virtual1000.firstPaintMs < 100,
      },
    },
    rustProcessMemory: {
      note: 'Rust 主进程私有提交量。集合树的 DOM 在 WebView2 渲染进程里，不在这里 —— 进程树内存由 PS1 量测台负责。',
      beforeMb: memBefore?.available ? memBefore.privateBytes / 1048576 : null,
      atHoldMb: memAtHold?.available ? memAtHold.privateBytes / 1048576 : null,
      afterHoldMb: memAfter?.available ? memAfter.privateBytes / 1048576 : null,
    },
    holdCondition: '1 工作区 + 3 标签 + 5000 节点集合树（已挂载、空闲）',
    holdMs: MEMORY_HOLD_MS,
    notMeasured: [
      '真实滚轮/触控板滚动：本面板用主线程写 scrollTop 驱动，不经过合成器线程，也不含输入事件延迟。真实滚轮在 Chromium 上可能走合成器，主线程驱动是更悲观的一侧。',
      '真实集合数据：节点由 synthTree 合成，名称长度均匀。真实集合里超长接口名会触发文本溢出计算，行内布局成本更高。',
      '增量加载：本次一次性把整棵扁平数组交给组件，未测「Rust 侧分页返回 + 滚动到底再取下一页」的路径（那是 v0.2 的实现）。',
      'macOS：WKWebView 的合成与滚动实现与 WebView2 不同，本报告数字不可套用。',
    ],
    cells: cells.value,
  }
}

onBeforeUnmount(() => {
  if (holdTimer) clearInterval(holdTimer)
  loafObserver?.disconnect()
})

// 无人值守：由 Rust 侧读 APOGEE_SPIKE_AUTORUN 环境变量决定（运行时开关，不是构建期）
//   s3      → 帧率矩阵 + 驻留
//   s3-mem  → 只驻留（干净进程，量内存用）
void ipc
  .spikeAutorun()
  .then((which) => {
    if (which === 's3') setTimeout(() => void runAll('full'), 400)
    else if (which === 's3-mem') setTimeout(() => void runAll('memoryOnly'), 400)
  })
  .catch(() => {
    /* 非 Tauri 环境下没有这个命令，手动点按钮即可 */
  })

// ── 展示辅助 ──────────────────────────────────────────────────────────

function ms(n: number | null | undefined, d = 1) {
  return n === null || n === undefined ? '—' : `${n.toFixed(d)}ms`
}
function num(n: number | null | undefined, d = 1) {
  return n === null || n === undefined ? '—' : n.toFixed(d)
}

defineExpose({ runAll })
</script>

<template>
  <div class="s3">
    <header>
      <h2>Spike S3 · 自研虚拟树帧率与首渲</h2>
      <p class="sub">
        验收：5000 节点首渲 &lt; 100ms、滚动 60fps（按掉帧率 &lt; 5% 判定）。
        顺带验 PRD 的「1000 请求集合树 &lt; 100ms」。
        <template v-if="display">
          显示器基线 {{ num(display.hz) }}Hz（标称 {{ display.nominalHz }}Hz，
          每帧预算 {{ ms(display.frameBudgetMs, 2) }}）。
        </template>
      </p>
      <div class="bar">
        <button type="button" :disabled="running" @click="runAll('full')">
          {{ running ? '运行中…' : '开始量测' }}
        </button>
        <button type="button" :disabled="running" @click="runAll('memoryOnly')">
          只测内存（干净驻留）
        </button>
        <span v-if="stage" class="stage">
          {{ stage }}<template v-if="holdRemaining > 0">（剩 {{ holdRemaining }}s）</template>
        </span>
        <span v-if="savedTo" class="saved">报告：{{ savedTo }}</span>
      </div>
    </header>

    <div class="body">
      <table v-if="cells.length" class="grid">
        <thead>
          <tr>
            <th>规模</th><th>实现</th><th>节点</th><th>实渲染</th>
            <th>数据准备</th><th>visible O(N)</th>
            <th class="hi">patch</th><th>layout</th><th>首帧</th>
            <th class="hi">fps</th><th>掉帧</th><th>p95</th>
            <th class="hi">单步成本</th><th>&gt;50ms 帧</th><th>展开 patch</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="c in cells" :key="`${c.scaleKey}-${c.impl}`" :class="{ ctrl: c.impl === 'full' }">
            <td>{{ c.scale }}</td>
            <td>{{ c.impl === 'virtual' ? '自研虚拟' : '对照·全量' }}</td>
            <td>{{ c.totalNodes }}</td>
            <td>{{ c.renderedRows }}</td>
            <td>{{ ms(c.dataPrepMs) }}</td>
            <td>{{ ms(c.visibleFilterMs, 2) }}</td>
            <td class="hi">{{ ms(c.patchMs, 2) }}</td>
            <td>{{ ms(c.layoutMs, 2) }}</td>
            <td>{{ ms(c.firstPaintMs) }}</td>
            <td class="hi">{{ num(c.scroll?.fps) }}</td>
            <td>{{ c.scroll ? `${num(c.scroll.droppedPct)}%` : '—' }}</td>
            <td>{{ ms(c.scroll?.p95Ms, 2) }}</td>
            <td class="hi">{{ ms(c.stepCost?.medianMs, 2) }}</td>
            <td>{{ c.loaf ? (c.loaf.supported ? c.loaf.count : 'n/a') : '—' }}</td>
            <td>{{ ms(c.togglePatchMedianMs, 2) }}</td>
          </tr>
        </tbody>
      </table>
      <p v-else class="hint">点「开始量测」。全程约 40 秒，最后有 14 秒内存驻留窗口。</p>

      <!-- 量测位。被测的是产品组件本身（Sidebar.vue），不是量测台里的副本。 -->
      <div class="slot" aria-label="Spike S3 量测位">
        <component
          :is="probe?.impl === 'full' ? S3FullTree : Sidebar"
          v-if="probe"
          :key="probe.key"
          ref="probeRef"
          :nodes="probe.nodes"
        />
        <p v-else class="hint">（量测位空闲）</p>
      </div>
    </div>
  </div>
</template>

<style scoped>
.s3 { display: flex; flex-direction: column; min-height: 0; flex: 1 1 auto; }
header { flex: 0 0 auto; padding: 8px 10px; border-bottom: 1px solid var(--a-border); }
h2 { margin: 0 0 4px; font: 600 var(--a-fs-base)/1.3 var(--a-font-ui); color: var(--a-text); }
.sub { margin: 0 0 6px; font: var(--a-fs-xs)/1.5 var(--a-font-ui); color: var(--a-text-dim); }
.bar { display: flex; align-items: center; gap: 10px; }
.bar button {
  height: var(--a-h-control-xs); padding: 0 10px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md);
  color: var(--a-text); font: inherit; cursor: pointer;
}
.bar button:disabled { opacity: 0.5; cursor: default; }
.stage, .saved { font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); color: var(--a-text-faint); }

.body { flex: 1 1 auto; display: flex; gap: 8px; min-height: 0; padding: 8px; }
.grid { flex: 1 1 auto; border-collapse: collapse; font: var(--a-fs-xs)/1.4 var(--a-font-mono); align-self: flex-start; }
.grid th, .grid td {
  padding: 2px 6px; border-bottom: 1px solid var(--a-border);
  text-align: right; white-space: nowrap; color: var(--a-text-dim);
}
.grid th { color: var(--a-text-faint); font-weight: 400; }
.grid th:first-child, .grid td:first-child,
.grid th:nth-child(2), .grid td:nth-child(2) { text-align: left; }
.grid .hi { color: var(--a-text); font-weight: 600; }
.grid tr.ctrl td { color: var(--a-text-faint); font-style: italic; }

/* 量测位给固定高度：视口高度直接决定要渲染多少行，不能让它随窗口漂。 */
.slot {
  flex: 0 0 auto; width: var(--a-w-sidebar-bench); height: 600px;
  display: flex; overflow: hidden;
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md);
}
.hint { margin: 0; padding: 6px; font: var(--a-fs-xs)/1.5 var(--a-font-ui); color: var(--a-text-faint); }
</style>
