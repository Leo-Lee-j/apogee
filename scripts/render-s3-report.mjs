#!/usr/bin/env node
/**
 * 把 S3 量测台产出的 s3.json 渲染成可读的 Markdown 报告。
 *
 * 分工与 S2 一致：量测由被测应用 + PowerShell 量测台做，报告渲染在 Node 里。
 * `scripts/measure-s3-windows.ps1` 跑完会自动调用本脚本，也可以单独重跑。
 */

import { readFileSync, writeFileSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const RESULTS = join(ROOT, 'spikes', 'results')
const r = JSON.parse(readFileSync(join(RESULTS, 's3.json'), 'utf8'))

const MB = 1048576

const n = (v, d = 1) => (v === null || v === undefined ? '—' : Number(v).toFixed(d))
const mb = (v) => (v === null || v === undefined ? '—' : `${(Number(v) / MB).toFixed(2)} MB`)
const ms = (v, d = 1) => (v === null || v === undefined ? '—' : `${Number(v).toFixed(d)} ms`)
const pctOf = (v, budget) =>
  v === null || v === undefined || !budget ? '—' : `${((Number(v) / budget) * 100).toFixed(0)}%`
const yn = (v) => (v ? '是' : '否')

const runs = r.runs ?? []
const pick = (config, mode) => runs.find((x) => x.config === config && x.mode === mode) ?? null
const fpsRuns = runs.filter((x) => x.mode === 'fps')
const memRuns = runs.filter((x) => x.mode === 'mem')
const primary = pick('gpu-on', 'fps') ?? fpsRuns[0]
const gpuOffFps = pick('gpu-off', 'fps')
// 内存一律走跨启动汇总，不看单轮 —— 单轮波动实测可达 8%
const summaryOf = (config, mode) =>
  (r.memorySummary ?? []).find((x) => x.config === config && x.mode === mode) ?? null
const primaryMem = summaryOf('gpu-on', 'mem') ?? summaryOf(memRuns[0]?.config, 'mem')
const gpuOffMem = summaryOf('gpu-off', 'mem')
/** 某个配置的 mem 轮里任意一轮（取 gpu 进程存在性等非数值信息） */
const memRunOf = (config) => memRuns.find((x) => x.config === config) ?? null
const cmp = r.gpuComparison
const failed = (r.gates ?? []).filter((g) => g.pass === false)
const pass = failed.length === 0

const acc = primary.frontend.acceptance
const firstPaint = acc['S3 首渲 5000 节点 < 100ms']
const scroll = acc['S3 滚动 5000 节点 60fps']
const tree1000 = acc['PRD 集合树 1000 请求 < 100ms']
const display = primary.frontend.display
const method = primary.frontend.method

const cells = primary.frontend.cells ?? []
const cell = (scaleKey, impl) => cells.find((c) => c.scaleKey === scaleKey && c.impl === impl) ?? null
const v1000 = cell('prd-1000', 'virtual')
const v5000 = cell('s3-5000', 'virtual')
const v20000 = cell('stress-20000', 'virtual')
const f1000 = cell('prd-1000', 'full')

const L = []

// ── 结论 ──────────────────────────────────────────────────────────────
L.push('# Spike S3 结果：自研虚拟树的帧率与首渲\n')
L.push('## 结论\n')
if (pass) {
  L.push(
    `**PASS。** 5000 节点首渲 **${ms(firstPaint.measuredMs)}**（预算 100ms，用掉 ` +
      `${pctOf(firstPaint.measuredMs, 100)}），滚动 **${n(scroll.fps)} fps** / 掉帧 ` +
      `**${n(scroll.droppedPct, 2)}%**（预算 < 5%）；PRD 的「1000 请求集合树 < 100ms」同时达标，` +
      `实测 **${ms(tree1000.measuredMs)}**。失败预案（改用组件库树控件或分页渲染）不需要启用。\n`,
  )
} else {
  L.push(
    `**FAIL。** ${failed.length} 项未通过：**${failed.map((g) => g.id).join('、')}**。` +
      `5000 节点首渲 ${ms(firstPaint.measuredMs)}，滚动 ${n(scroll.fps)} fps / 掉帧 ${n(scroll.droppedPct, 2)}%。\n`,
  )
}
if (primaryMem) {
  L.push(
    `**顺带补上 PRD §5.1 一直缺的那个量测条件。** S1 量的是空壳（1 个空标签、无集合），` +
      `本 Spike 的驻留条件是「1 工作区 + 3 标签 + 5000 节点集合树」—— 比 PRD 声明的「1 工作区、3 标签」` +
      `还多一棵树。干净启动 ${primaryMem.rounds} 轮，进程树私有常驻中位 ` +
      `**${mb(primaryMem.treePrivateWorkingSetMedianBytes)}**、最差 ` +
      `**${mb(primaryMem.treePrivateWorkingSetMaxBytes)}**（跨轮波动 ${n(primaryMem.launchVariancePercent)}%），` +
      `其中 Rust 侧 ${mb(primaryMem.rootPrivateWorkingSetMedianBytes)}。` +
      `对 PRD 的 150MB 预算，这是 **${((primaryMem.treePrivateWorkingSetMaxBytes / (150 * MB)) * 100).toFixed(0)}%** —— ` +
      `**默认配置下预算不可达**，而超出的部分不在我们的代码里（见「发现」的角色分解）。\n`,
  )
}
if (cmp && gpuOffMem) {
  const offPeak = gpuOffMem.treePrivateWorkingSetMaxBytes
  L.push(
    `**关于内存门槛（03 §10.4 选项 b）：关 GPU 能省 ${mb(Math.abs(cmp.memorySavedBytes))}` +
      `（${n(Math.abs(cmp.memorySavedPercent))}%），` +
      `${offPeak < 150 * MB ? `足以把内存压回 150MB 预算内（${mb(offPeak)}）` : `但仍压不回 150MB 预算内（${mb(offPeak)}）`}，` +
      `本 Spike 的滚动场景没测到帧率代价。** ` +
      `掉帧率 ${n(cmp.droppedPctOn, 2)}% → **${n(cmp.droppedPctOff, 2)}%**（fps ${n(cmp.fpsOn)} → ${n(cmp.fpsOff)}），` +
      `单步主线程成本 ${ms(cmp.stepCostMedianMsOn, 2)} → ${ms(cmp.stepCostMedianMsOff, 2)}。` +
      `传 \`--disable-gpu --disable-gpu-compositing\` 之后 gpu-process ` +
      `**${cmp.gpuProcessStillPresentWhenDisabled ? '仍然存在，只是从 ' + mb(memRunOf('gpu-on')?.gpu.gpuProcessPrivateWorkingSetBytes) + ' 缩到 ' + mb(memRunOf('gpu-off')?.gpu.gpuProcessPrivateWorkingSetBytes) : '已经消失'}**。` +
      `⚠️ **这个结论只覆盖「文本行列表滚动」一种负载**，而开关是全局的 —— 适用范围见专门一节。\n`,
  )
}
L.push(
  `环境：${r.platform.os} ${r.platform.osVersion} · ${r.platform.cpu} · ${r.platform.logicalCores} 逻辑核 · ` +
    `内存 ${(r.platform.totalMemoryBytes / 1024 ** 3).toFixed(1)} GB · WebView2 Runtime ${r.platform.webview2Runtime ?? '未知'} · ` +
    `node ${r.platform.node}。量测时间 ${r.at}，全程 ${n(r.harness.measurementSeconds)}s。`,
)
L.push(
  `显示器基线：实测 rAF 中位间隔 ${ms(display.frameBudgetMs, 2)} → **${n(display.hz)} Hz**` +
    `（标称 ${display.nominalHz}Hz，${display.sampleCount} 帧样本，跨度 ${ms(display.minMs, 2)}～${ms(display.maxMs, 2)}）。\n`,
)
if (r.harness.hotConfigs?.length) {
  L.push(
    `> **量测环境有背景负载**：配置 ${r.harness.hotConfigs.join('、')} 在 CPU 未降到 ` +
      `${r.harness.cpuGateMaxPercent}% 以下时就开跑。帧率对 CPU 竞争极敏感，这些轮次的帧率应视为**下限**。\n`,
  )
}

// ── 口径 ──────────────────────────────────────────────────────────────
L.push('## 三条量测口径\n')
L.push(
  `1. **帧率按掉帧率判定，不按「fps ≥ 60」判定。** 本机显示器 ${n(display.hz)}Hz，` +
    `也就是说 ${display.nominalHz} fps 就是物理上限 —— 用「fps ≥ 60」当门槛，` +
    `一个一帧不掉的实现也可能因采样噪声报出 59.4 而「不达标」。所以先测显示器实际 rAF 节奏` +
    `（${ms(display.frameBudgetMs, 2)}/帧）作为基准，再按相对该节奏掉了多少帧判定，阈值 5%。` +
    `这也是 Chromium 自己的 smoothness 口径。\n`,
)
L.push(
  `2. **滚动由主线程驱动**：每帧写 \`scrollTop\`，步长 ${method.scrollStepRows} 行` +
    `（${method.scrollStepPx}px，一次快速 fling 的量级），到边界折返，采 ${method.scrollFrames} 帧。` +
    `真实滚轮在 Chromium 上可以走合成器线程，**主线程驱动是更悲观的一侧**，而且它才会每帧触发` +
    `我们的 slice 重算 + 行 DOM 更新 —— 那正是要测的东西。固定「行/帧」而不是「总高度/帧数」，` +
    `是为了让三个档位每帧工作量相同：这样帧时间若随节点数上升，一定是虚拟化本身的问题。\n`,
)
L.push(
  '3. **含 vsync 等待的数字和不含的分开报。** `patch`（数据就绪 → Vue 打完 DOM）与 `layout`' +
    '（读 `offsetHeight` 强制同步排版）不受显示器节奏影响，是「我们的代码有多快」；' +
    '`首帧`（→ 像素上屏）含最多 2 个 vsync 的等待，是「用户等了多久」。\n',
)
L.push(
  `   **这条不是学术洁癖 —— 不拆就会读错。** 本次自研实现在三个档位的首帧都是 ` +
    `${[v1000, v5000, v20000].map((c) => ms(c?.firstPaintMs)).join(' / ')}，` +
    `看着像「与规模无关」，但那只是 2 个 vsync（${ms((display.frameBudgetMs ?? 0) * 2, 1)}）的等待把差异全盖住了。` +
    `真正的计算成本是 patch ${[v1000, v5000, v20000].map((c) => ms(c?.patchMs, 2)).join(' / ')}。` +
    `同理，帧间隔（p50 一律 ${ms(v5000?.scroll?.p50Ms, 2)}）也是**饱和**的：` +
    `只要单帧工作量低于 ${ms(display.frameBudgetMs, 2)}，任何实现都会报出相同的 fps 和 0% 掉帧。` +
    `所以另外测了「单步成本」（\`scrollTo\` → Vue patch → 强制 layout，不含 paint/composite），` +
    `它才能回答「还剩多少余量」。\n`,
)
L.push(
  `被测对象是**产品组件本身**：\`${primary.frontend.subject.component}\`` +
    `（行高 ${primary.frontend.subject.rowHeightPx}px、上下缓冲 ${primary.frontend.subject.overscanRows} 行，` +
    `参数与数据形状在 \`${primary.frontend.subject.sharedModule}\`）。对照组是 \`${primary.frontend.subject.control}\`。\n`,
)

// ── 验收对照 ──────────────────────────────────────────────────────────
L.push('## 验收对照（默认配置 / GPU 开）\n')
L.push('| 验收项 | 归属 | 标准 | 实测 | 预算占用 | 结果 |')
L.push('| --- | --- | --- | --- | --- | --- |')
for (const g of r.gates ?? []) {
  const measured = g.unit === 'ms' ? ms(g.measured) : `${n(g.measured, 2)}${g.unit}`
  L.push(
    `| ${g.id} | ${g.source} | ${g.criterion} | **${measured}** | ${pctOf(g.measured, g.budget)} | ` +
      `${g.pass === null ? '未量测' : g.pass ? 'PASS' : '**FAIL**'} |`,
  )
}
L.push('')
L.push(
  '门禁只按默认配置（GPU 开）判定 —— 那是产品要发布的形态。GPU 关是 03 §10.4 里待评估的**选项**，' +
    '它的数字进报告、参与决策，但不作门禁：让候选优化方案的表现决定 Spike 是否通过，等于把结论和选项搅在一起。\n',
)

// ── 矩阵明细 ──────────────────────────────────────────────────────────
for (const run of fpsRuns) {
  L.push(`## 逐档明细 · ${run.label}\n`)
  L.push('计算成本（不含 vsync）| 用户可感知延迟（含 vsync）| 滚动：')
  L.push('')
  L.push(
    '| 规模 | 实现 | 节点 | 实渲染 | 数据准备 | visible O(N) | **patch** | layout | 首帧 | **fps** | 掉帧 | p95 帧间隔 | **单步成本** | 单步 p95 | >50ms 帧 | 展开 patch | 展开→上屏 |',
  )
  L.push(
    '| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |',
  )
  for (const c of run.frontend.cells ?? []) {
    const s = c.scroll
    L.push(
      `| ${c.scale} | ${c.impl === 'virtual' ? '自研虚拟' : '_对照·全量_'} | ${c.totalNodes} | ` +
        `${c.renderedRows} | ${ms(c.dataPrepMs, 2)} | ${ms(c.visibleFilterMs, 2)} | **${ms(c.patchMs, 2)}** | ` +
        `${ms(c.layoutMs, 2)} | ${ms(c.firstPaintMs)} | **${n(s?.fps)}** | ${s ? `${n(s.droppedPct, 2)}%` : '—'} | ` +
        `${ms(s?.p95Ms, 2)} | **${ms(c.stepCost?.medianMs, 2)}** | ${ms(c.stepCost?.p95Ms, 2)} | ` +
        `${c.loaf ? (c.loaf.supported ? c.loaf.count : 'n/a') : '—'} | ${ms(c.togglePatchMedianMs, 2)} | ${ms(c.toggleMedianMs)} |`,
    )
  }
  L.push('')
  const truncated = (run.frontend.cells ?? []).filter((c) => c.scroll?.truncated)
  if (truncated.length > 0) {
    L.push(
      `注：${truncated.map((c) => `${c.scale}/${c.impl === 'virtual' ? '虚拟' : '全量'}`).join('、')} ` +
        `的滚动采样因单帧过慢触到墙钟上限提前收尾（实际采到 ` +
        `${truncated.map((c) => c.scroll.frames).join('、')} 帧，目标 ${method.scrollFrames} 帧）。` +
        `帧时间分布不受截断影响，但帧数必须诚实报出来。\n`,
    )
  }
  const errored = (run.frontend.cells ?? []).filter((c) => c.error)
  if (errored.length > 0) {
    L.push('量测失败的格子：')
    for (const c of errored) L.push(`- ${c.scale} / ${c.impl}：\`${c.error}\``)
    L.push('')
  }
}

// ── 内存 ──────────────────────────────────────────────────────────────
L.push('## 负载下的进程树内存\n')
L.push(
  '驻留条件：**1 工作区 + 3 标签 + 5000 节点集合树（已挂载、空闲）**。' +
    'S1 量的是空壳（1 个空标签、无集合），这里补上了 PRD §5.1 声明的「1 工作区、3 标签」' +
    '外加一棵 5000 节点的集合树 —— 比 PRD 的量测条件更重，所以是上界。\n',
)
L.push(
  '**每种 GPU 配置起两次进程**：`mem` 轮干净启动直接进驻留，是稳态；`fps` 轮跑完帧率矩阵再驻留，' +
    '含一次重负载会话的残留（5 次组件创建/销毁，其中两次铺过 1040 和 5000 行真实 DOM）。' +
    '**定门槛只能用 `mem` 轮** —— 用 `fps` 轮等于把量测台自己的开销算进产品预算。两个都报，' +
    '差值就是「一次重会话之后内存回不回来」的答案。\n',
)
L.push('口径与 S1 完全一致（共用 `scripts/lib/ProcessTreeMemory.psm1`）：私有工作集 = `Working Set - Private`，等于任务管理器「内存」列。\n')
L.push(
  `**内存跨 ${r.config.memoryRounds} 次启动取值**，门槛按最差观测值定。单次启动不够：` +
    `同一份二进制两次启动的驻留内存实测能差 ` +
    `${n(Math.max(...(r.memorySummary ?? []).map((x) => x.launchVariancePercent ?? 0)))}%` +
    `（WebView2 的 GPU/着色器缓存状态不同）。\n`,
)
L.push('| 配置 | 轮次 | 启动次数 | 进程数 | 树私有常驻 中位 | 树私有常驻 最差 | 跨轮波动 | Rust 主进程 |')
L.push('| --- | --- | --- | --- | --- | --- | --- | --- |')
for (const s of r.memorySummary ?? []) {
  const label = s.config === 'gpu-on' ? 'GPU 开（默认）' : 'GPU 关'
  const modeText = s.mode === 'mem' ? '**干净驻留（稳态）**' : '_跑完帧率矩阵后_'
  L.push(
    `| ${label} | ${modeText} | ${s.rounds} | ${s.processCount} | **${mb(s.treePrivateWorkingSetMedianBytes)}** | ` +
      `**${mb(s.treePrivateWorkingSetMaxBytes)}** | ${n(s.launchVariancePercent)}% | ${mb(s.rootPrivateWorkingSetMedianBytes)} |`,
  )
}
L.push('')
const fpsMemSummary = summaryOf('gpu-on', 'fps')
if (primaryMem && fpsMemSummary) {
  const residue =
    fpsMemSummary.treePrivateWorkingSetMedianBytes - primaryMem.treePrivateWorkingSetMedianBytes
  const rendererAfter = fpsMemSummary.roleBreakdown.find((x) => x.role === 'renderer')?.privateWorkingSetBytes
  const rendererClean = primaryMem.roleBreakdown.find((x) => x.role === 'renderer')?.privateWorkingSetBytes
  L.push(
    `默认配置下，跑完帧率矩阵之后的驻留内存比干净驻留高 **${mb(residue)}**` +
      `（${mb(fpsMemSummary.treePrivateWorkingSetMedianBytes)} vs ${mb(primaryMem.treePrivateWorkingSetMedianBytes)}），` +
      `几乎全部落在 \`renderer\`（${mb(rendererAfter)} vs ${mb(rendererClean)}）。` +
      `十几秒的驻留窗口内没有回落 —— 这不能断定是泄漏（V8 的 GC 不保证及时归还，分离 DOM 也要等下一轮 major GC），` +
      `但足以说明**「反复创建销毁大列表」这条路径的内存不会立刻还回来**。` +
      `标签虚拟化（03 §4.2）要按这个前提设计：把标签转 Cold 时卸掉 DOM，不等于内存马上回到基线。\n`,
  )
}
for (const s of (r.memorySummary ?? []).filter((x) => x.mode === 'mem')) {
  L.push(`### 内存花在哪 · ${s.config === 'gpu-on' ? 'GPU 开（默认）' : 'GPU 关'}（干净驻留）\n`)
  L.push('| 角色 | 进程数 | 私有常驻 | 占进程树 |')
  L.push('| --- | --- | --- | --- |')
  for (const role of s.roleBreakdown ?? []) {
    L.push(
      `| \`${role.role}\` | ${role.instances} | **${mb(role.privateWorkingSetBytes)}** | ` +
        `${pctOf(role.privateWorkingSetBytes, s.treePrivateWorkingSetMedianBytes)} |`,
    )
  }
  L.push('')
}
L.push(
  `Rust 侧自报的私有提交量在干净驻留期是 ${n(memRunOf('gpu-on')?.frontend.rustProcessMemory.atHoldMb, 2)} MB。` +
    '集合树的 DOM 在 WebView2 渲染进程里，不在 Rust 进程里 —— 这也是为什么「性能自检」面板' +
    '必须报整个进程树而不能只报 Rust 进程（PRD §5.2）。\n',
)
L.push(
  `旁证：驻留时读到的进程树 **WorkingSet 峰值**（含共享页的口径，S1 实测放大 2.99×）为 ` +
    `${(r.memorySummary ?? [])
      .map((s) => {
        const one = runs.find((x) => x.config === s.config && x.mode === s.mode)
        return `${s.config}/${s.mode} ${mb(one?.memory.treePeakWorkingSetBytes)}`
      })
      .join('、')}。` +
    `Windows 没有「私有工作集峰值」计数器，所以它只能回答「过程中有没有出现远高于驻留期的尖峰」，` +
    `**不参与门禁，也不能和上面的私有常驻数字并列比较**。\n`,
)

// ── GPU 对照 ──────────────────────────────────────────────────────────
if (cmp && gpuOffMem && gpuOffFps && primaryMem) {
  L.push('## 关掉硬件加速值不值（03 §10.4 选项 b）\n')
  L.push(
    'S1 发现空闲内存里单个 `gpu-process` 就占 67 MB（46%），于是 §10.4 留了一条「从 WebView2 侧找真实节省」' +
      '的路，并注明「关硬件加速能省，但会牺牲滚动帧率，**须先做 S3**」。这一节就是那个答案。\n',
  )
  L.push(
    `内存取 \`mem\` 轮（干净驻留，${primaryMem.rounds} 次启动的最差观测值），帧率取 \`fps\` 轮。` +
      '混着比会拿含残留的内存去和稳态比，节省量就是假的。\n',
  )
  L.push('| 指标 | GPU 开 | GPU 关 | 差值 |')
  L.push('| --- | --- | --- | --- |')
  L.push(
    `| 树私有常驻 最差（干净驻留） | **${mb(primaryMem.treePrivateWorkingSetMaxBytes)}** | ` +
      `**${mb(gpuOffMem.treePrivateWorkingSetMaxBytes)}** | ` +
      `${cmp.memorySavedBytes >= 0 ? '省 ' : '多 '}${mb(Math.abs(cmp.memorySavedBytes))}（${n(Math.abs(cmp.memorySavedPercent))}%） |`,
  )
  L.push(
    `| 树私有常驻 中位（干净驻留） | ${mb(primaryMem.treePrivateWorkingSetMedianBytes)} | ` +
      `${mb(gpuOffMem.treePrivateWorkingSetMedianBytes)} | ` +
      `省 ${mb(primaryMem.treePrivateWorkingSetMedianBytes - gpuOffMem.treePrivateWorkingSetMedianBytes)} |`,
  )
  L.push(
    `| 进程数 | ${primaryMem.processCount} | ${gpuOffMem.processCount} | ` +
      `${gpuOffMem.processCount - primaryMem.processCount} |`,
  )
  L.push(
    `| \`gpu-process\` 私有常驻 | ${mb(memRunOf('gpu-on')?.gpu.gpuProcessPrivateWorkingSetBytes)} | ` +
      `${mb(memRunOf('gpu-off')?.gpu.gpuProcessPrivateWorkingSetBytes)} | ` +
      `${memRunOf('gpu-off')?.gpu.gpuProcessPresent ? '进程仍在' : '进程消失'} |`,
  )
  L.push(`| 滚动 fps（5000 节点） | **${n(cmp.fpsOn)}** | **${n(cmp.fpsOff)}** | ${n(cmp.fpsDelta)} |`)
  L.push(
    `| 掉帧率（5000 节点） | **${n(cmp.droppedPctOn, 2)}%** | **${n(cmp.droppedPctOff, 2)}%** | ` +
      `${n(cmp.droppedPctDelta, 2)} 个百分点 |`,
  )
  L.push(
    `| 单步主线程成本（5000 节点） | ${ms(cmp.stepCostMedianMsOn, 2)} | ${ms(cmp.stepCostMedianMsOff, 2)} | ` +
      `${ms(cmp.stepCostMedianMsOff - cmp.stepCostMedianMsOn, 2)} |`,
  )
  for (const key of ['prd-1000', 's3-5000', 'stress-20000']) {
    const a = (primary.frontend.cells ?? []).find((c) => c.scaleKey === key && c.impl === 'virtual')
    const b = (gpuOffFps.frontend.cells ?? []).find((c) => c.scaleKey === key && c.impl === 'virtual')
    if (!a || !b) continue
    L.push(`| 首渲 patch · ${a.scale} | ${ms(a.patchMs, 2)} | ${ms(b.patchMs, 2)} | ${ms(b.patchMs - a.patchMs, 2)} |`)
  }
  L.push('')
  const offRun = memRunOf('gpu-off')
  L.push(
    `开关是否真的生效（不能相信环境变量，必须看命令行）：` +
      `${(offRun?.gpu.flagsObserved ?? []).map((f) => `\`${f.flag}\` ${yn(f.presentInCommandLine)}`).join('、')}；` +
      `进程树里 \`gpu-process\` **${offRun?.gpu.gpuProcessPresent ? '仍然存在' : '已消失'}**。\n`,
  )
  L.push('### 这个结论的适用范围（重要）\n')
  L.push(
    '**本节只证明了一件事：在「文本行列表滚动」这一种负载下，关掉硬件加速没有测到帧率代价。**' +
      '不能据此推广到「关 GPU 对本产品无害」。三条具体限制：\n',
  )
  L.push(
    `1. **本量测的负载对合成器很轻。** 自研虚拟树滚动时只更新 ${v5000?.renderedRows ?? '几十'} 行纯文本 DOM，` +
      `单步主线程成本 ${ms(cmp.stepCostMedianMsOn, 2)}，占一帧预算 ${pctOf(cmp.stepCostMedianMsOn, display.frameBudgetMs)}。` +
      `真正吃合成能力的是 CodeMirror 滚动大文档、响应体树的大面积重绘、以及未来的图表 —— 这些都没测。`,
  )
  L.push(
    '2. **软件光栅化的代价随重绘面积增长，而窗口大小是用户定的。** 本次窗口是默认 1280×820，' +
      '量测位只有 240×600。全屏 4K 下的软件合成成本可能是另一个量级。',
  )
  L.push(
    '3. **`--disable-gpu` 是全局开关，不能只对集合树生效。** 一旦关掉，整个应用（含 CodeMirror、' +
      '未来的 WebGL 图表）都退到软件合成。收益是全局的，代价也是全局的。\n',
  )
}

// ── 发现 ──────────────────────────────────────────────────────────────
L.push('## 发现\n')
const findings = []

if (v1000 && v5000 && v20000) {
  const patches = [v1000.patchMs, v5000.patchMs, v20000.patchMs]
  const filterRatio = v20000.visibleFilterMs / Math.max(v1000.visibleFilterMs, 1e-6)
  findings.push(
    `**成本与规模解耦：DOM 量恒定，只有一趟 O(N) 扫描随节点数增长。** ` +
      `1000 / 5000 / 20000 三档实渲染行数都是 ${v5000.renderedRows} 行 —— 视口高度定死了这个数，` +
      `节点数完全不影响它。随 N 增长的只有 \`visible\` 那趟过滤：` +
      `${ms(v1000.visibleFilterMs, 3)} / ${ms(v5000.visibleFilterMs, 3)} / ${ms(v20000.visibleFilterMs, 3)}` +
      `（每档重复 ${method.visibleFilterRepeats} 次取平均），19 倍节点数换来 ${n(filterRatio)} 倍耗时 —— 线性，符合预期，` +
      `而 20000 节点的绝对值仍只占一帧预算的 ${pctOf(v20000.visibleFilterMs, display.frameBudgetMs)}。` +
      `首渲 patch 三档 ${patches.map((x) => ms(x, 2)).join(' / ')}，在噪声范围内持平` +
      `（跨度 ${ms(Math.max(...patches) - Math.min(...patches), 2)}，且不随 N 单调）。` +
      `这与 S2 的「Range 取首屏分片耗时与 body 体积无关」是同一个架构模式：` +
      `**把成本与规模解耦，而不是把成本优化小。**`,
  )
}
if (v5000?.stepCost) {
  findings.push(
    `**滚动的余量很大，但这一点必须靠单步成本才能看出来。** 三档的帧间隔 p50 一律 ` +
      `${ms(v5000.scroll.p50Ms, 2)}、掉帧一律 0% —— 那是**显示器节奏的饱和读数**，不是我们的性能。` +
      `不受 vsync 影响的单步主线程成本是 ${[v1000, v5000, v20000].map((c) => ms(c?.stepCost?.medianMs, 2)).join(' / ')}` +
      `（p95 ${[v1000, v5000, v20000].map((c) => ms(c?.stepCost?.p95Ms, 2)).join(' / ')}），` +
      `占 ${ms(display.frameBudgetMs, 2)} 帧预算的 ${pctOf(v5000.stepCost.medianMs, display.frameBudgetMs)}。` +
      `**余量约 ${n(display.frameBudgetMs / Math.max(v5000.stepCost.medianMs, 0.01))} 倍**，且不随节点数变化` +
      `（滚动只重算 \`slice\`，是 O(视口)；\`visible\` 被计算缓存挡住不会重跑）。` +
      `教训是通用的：**在贴着 vsync 的场景里，fps 和帧间隔都是饱和指标，只能证伪不能定量。**`,
  )
}
if (f1000 && v1000) {
  const f5000 = cell('s3-5000', 'full')
  const parts = [
    `**对照组量化了虚拟化的收益，而且收益在首渲不在滚动。** ` +
      `1040 行时全量 \`v-for\` 铺 ${f1000.renderedRows} 行 DOM、patch ${ms(f1000.patchMs, 2)}，` +
      `自研虚拟只铺 ${v1000.renderedRows} 行、patch ${ms(v1000.patchMs, 2)}` +
      `（DOM 节点差 ${n(f1000.renderedRows / Math.max(v1000.renderedRows, 1))} 倍，patch 差 ` +
      `${n(f1000.patchMs / Math.max(v1000.patchMs, 0.01))} 倍）。`,
  ]
  if (f5000 && v5000) {
    parts.push(
      `5000 行时差距拉开到 patch ${ms(f5000.patchMs, 2)} vs ${ms(v5000.patchMs, 2)}` +
        `（${n(f5000.patchMs / Math.max(v5000.patchMs, 0.01))} 倍），首帧 ${ms(f5000.firstPaintMs)} vs ` +
        `${ms(v5000.firstPaintMs)} —— 全量渲染在这一档已经击穿 100ms 首渲预算。`,
    )
  }
  parts.push(
    `但**滚动**两者都是 0% 掉帧：已经布局好的长列表滚动由合成器处理，主线程不参与。` +
      `所以虚拟化真正买到的是**首次渲染时间和内存**，不是滚动帧率 —— 这一点和直觉相反，` +
      `也说明 02 文档 §7.5 第 4 条把集合树列为「必须自研」的理由应当写成首渲与内存，而不是「滚动更流畅」。`,
  )
  findings.push(parts.join(''))
}
if (v20000) {
  findings.push(
    `**展开/折叠是唯一随规模线性增长的交互路径，必须单独盯。** 它走 \`all\` 换引用 → \`visible\` 全量重算，` +
      `是 O(N)。计算成本（patch，不含 vsync）三档中位 ` +
      `${[v1000, v5000, v20000].map((c) => ms(c?.togglePatchMedianMs, 2)).join(' / ')}，最长 ` +
      `${[v1000, v5000, v20000].map((c) => ms(c?.togglePatchMaxMs, 2)).join(' / ')}；` +
      `到像素上屏则一律 ${ms(v5000.toggleMedianMs)}（＝2 个 vsync，又是饱和读数）。` +
      `20000 节点下最长 ${ms(v20000.togglePatchMaxMs, 2)}，仍在一帧之内，` +
      `所以当前实现不需要为展开/折叠做增量更新。**但这是三条路径里唯一会随集合增大而变慢的**，` +
      `等真实集合突破十万节点就要重新量。`,
  )
}
const loafCells = cells.filter((c) => c.loaf?.supported)
if (loafCells.length > 0) {
  const virtualLoaf = loafCells.filter((c) => c.impl === 'virtual')
  const fullLoaf = loafCells.filter((c) => c.impl === 'full')
  const total = virtualLoaf.reduce((a, c) => a + c.loaf.count, 0)
  const fullTotal = fullLoaf.reduce((a, c) => a + c.loaf.count, 0)
  findings.push(
    `**Long Animation Frame：自研实现三档共 ${total} 个超 50ms 的动画帧，对照组 ${fullTotal} 个。** ` +
      `LoAF 只上报耗时超 50ms 的帧，所以它是一个「有没有出现卡顿」的二值信号，` +
      `${total === 0 ? '一个都没有比平均 fps 更能说明滚动过程里没有长帧' : `最长 ${ms(Math.max(...virtualLoaf.map((c) => c.loaf.maxDurationMs ?? 0)), 2)}`}。` +
      `${fullTotal > 0 ? '对照组那几个落在首次渲染（铺 5000 行 DOM）而不是滚动阶段。' : ''}`,
  )
}
// PRD §5.1 的预算与 S1 空壳基线，用于把 S3 的数字放回预算语境里。
const PRD_BUDGET_BYTES = 150 * MB
const S1_SHELL = { medianBytes: 153829376, maxBytes: 172388352, rootBytes: 3854336, gpuBytes: 70273024, rendererBytes: 24356864 }

if (primaryMem) {
  const rss = primaryMem
  const roleOf = (role) => rss.roleBreakdown.find((x) => x.role === role)?.privateWorkingSetBytes ?? 0
  const deltas = [
    ['gpu-process', roleOf('gpu-process') - S1_SHELL.gpuBytes],
    ['renderer', roleOf('renderer') - S1_SHELL.rendererBytes],
    ['host(rust)', rss.rootPrivateWorkingSetMedianBytes - S1_SHELL.rootBytes],
  ].sort((a, b) => b[1] - a[1])
  findings.push(
    `**PRD 声明的量测条件第一次被真正量到：${mb(rss.treePrivateWorkingSetMaxBytes)}，是 150MB 预算的 ` +
      `${((rss.treePrivateWorkingSetMaxBytes / PRD_BUDGET_BYTES) * 100).toFixed(0)}%。** ` +
      `S1 只量了空壳（1 个空标签、无集合），中位 ${mb(S1_SHELL.medianBytes)}；` +
      `本次干净驻留（1 工作区 + 3 标签 + 5000 节点树）${rss.rounds} 轮启动，中位 ` +
      `${mb(rss.treePrivateWorkingSetMedianBytes)}、最差 ${mb(rss.treePrivateWorkingSetMaxBytes)}，比空壳多 ` +
      `${mb(rss.treePrivateWorkingSetMedianBytes - S1_SHELL.medianBytes)}。` +
      `增量按角色分解：${deltas.map(([k, v]) => `\`${k}\` ${v >= 0 ? '+' : ''}${mb(v)}`).join('、')} ——` +
      `**最大的一块在 \`gpu-process\`，不在我们的代码里**；Rust 侧那 ` +
      `${mb(rss.rootPrivateWorkingSetMedianBytes - S1_SHELL.rootBytes)} 是 spool 目录与 SpanIndex 缓存的基础结构。` +
      `含义：03 §4.2 那套「响应体不进内存 / 索引 LRU」管的仍然是 Rust 那几 MB，` +
      `而预算的分母由 WebView2 决定 —— 与 S1 的结论一致，只是这次是在真实负载下复现的。`,
  )
}
if (cmp && gpuOffMem && primaryMem) {
  const offPeak = gpuOffMem.treePrivateWorkingSetMaxBytes
  const withinBudget = offPeak < PRD_BUDGET_BYTES
  const onGpuBytes = memRunOf('gpu-on')?.gpu.gpuProcessPrivateWorkingSetBytes ?? 0
  const offGpuBytes = memRunOf('gpu-off')?.gpu.gpuProcessPrivateWorkingSetBytes ?? 0
  const gpuFinding = cmp.gpuProcessStillPresentWhenDisabled
    ? `传 \`--disable-gpu --disable-gpu-compositing\` 之后 **\`gpu-process\` 并没有消失** —— ` +
      `现代 Chromium 会退到 SwiftShader 软件光栅化，进程照旧存在，只是从 ` +
      `${mb(onGpuBytes)} 缩到 ${mb(offGpuBytes)}（省 ${mb(onGpuBytes - offGpuBytes)}）。` +
      `**「关掉硬件加速就少一个进程」是错的**，这也是为什么量测台要断言实际观测到的进程树而不是相信开关。`
    : `传 \`--disable-gpu --disable-gpu-compositing\` 之后 \`gpu-process\` 确实消失了。`
  findings.push(
    `**关硬件加速能省 ${mb(Math.abs(cmp.memorySavedBytes))}（${n(Math.abs(cmp.memorySavedPercent))}%），` +
      `${withinBudget ? `而且真的能把内存压回 150MB 预算内（${mb(offPeak)}）` : `但仍压不回 150MB 预算内（${mb(offPeak)}）`}。** ` +
      `${gpuFinding} 进程树私有常驻最差值 ${mb(primaryMem.treePrivateWorkingSetMaxBytes)} → ${mb(offPeak)}，` +
      `掉帧率 ${n(cmp.droppedPctOn, 2)}% → ${n(cmp.droppedPctOff, 2)}%，单步主线程成本 ` +
      `${ms(cmp.stepCostMedianMsOn, 2)} → ${ms(cmp.stepCostMedianMsOff, 2)}（差 ${ms(cmp.stepCostMedianMsOff - cmp.stepCostMedianMsOn, 2)}）。` +
      `${
        withinBudget
          ? `**所以选项 (b) 技术上是可行的 —— 但可行不等于该做。** 本 Spike 的负载对合成器极轻` +
            `（滚动时只更新 ${v5000?.renderedRows} 行纯文本，单步成本占一帧的 ${pctOf(cmp.stepCostMedianMsOn, display.frameBudgetMs)}），` +
            `而 \`--disable-gpu\` 是全局开关：CodeMirror 滚动大文档、响应体树大面积重绘、全屏 4K 下的软件合成` +
            `全都会被牵连，这些一个都没测。用「达标」去换一项影响面覆盖整个 UI 而只验证过一种负载的降级，` +
            `是拿一个测过的指标去赌几个没测过的指标。**建议：不采用 (b) 作为达标手段**，` +
            `而是把它记为一项已量化的可选优化（省 ${mb(Math.abs(cmp.memorySavedBytes))}），` +
            `等 CodeMirror 与响应体树的合成负载也量过之后再决定是否提供为用户可选开关。`
          : `**这条直接否掉了选项 (b)**：不能靠关 GPU 把内存拉回预算内。`
      }`,
  )
}
findings.forEach((f, i) => {
  L.push(`${i + 1}. ${f}\n`)
})

// ── 未量测 ────────────────────────────────────────────────────────────
L.push('## 未量测 / 边界\n')
// 量测台与被测应用各自申报的边界，去重后合并 —— 两边都可能想到对方没想到的。
const boundaries = [...(r.notMeasured ?? []), ...(primary.frontend.notMeasured ?? [])]
const seenPrefix = new Set()
for (const item of boundaries) {
  // 两边可能对同一条边界各写一句，按「冒号前的主题」去重
  const key = item.split(/[：:]/)[0]
  if (seenPrefix.has(key)) continue
  seenPrefix.add(key)
  L.push(`- ${item}`)
}
L.push('')

L.push('---\n')
L.push(
  '原始数据：`spikes/results/s3.json`。复现：`pnpm spike:s3`（复用已有 release 产物用 `pnpm spike:s3:nobuild`）。\n',
)
L.push(`UA：\`${primary.frontend.userAgent}\``)

writeFileSync(join(RESULTS, 's3-report.md'), L.join('\n'), 'utf8')
console.log(pass ? 'S3: PASS' : `S3: FAIL（${failed.map((g) => g.id).join('、')}）`)
console.log('报告：spikes/results/s3-report.md')
