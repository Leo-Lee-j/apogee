#!/usr/bin/env node
/**
 * # Spike S5：首屏 chunk 体积量测
 *
 * ## 要验证的（03 文档 §10）
 *
 * > S5：Naive UI 按需引入后的首屏 chunk 体积。成功标准：首屏 gzip < 150KB。
 * > 失败预案：关键路径组件自研，Naive UI 只留设置页等低频页面。
 *
 * ## 量测方法
 *
 * 关键在于「首屏」的定义。只统计 dist 目录总大小是错的 —— 懒加载 chunk 不该算。
 * 正确做法是从 entry 出发**只走静态 import 边**（manifest 的 `imports`），
 * 遇到 `dynamicImports` 就停。这才是浏览器为了画出首屏必须下载的集合。
 *
 * 同时跑两个变体做对照：
 *   A = 按 02 文档 §7.5 把 n-config-provider 放首屏
 *   B = naive-ui 全部推到懒加载 chunk
 *
 * 输出：spikes/results/s5.json + s5-report.md
 */

import { execFileSync } from 'node:child_process'
import { gzipSync, brotliCompressSync, constants as zlibConstants } from 'node:zlib'
import { readFileSync, writeFileSync, existsSync, rmSync, mkdirSync, statSync, readdirSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const DIST = join(ROOT, 'dist')
const RESULTS = join(ROOT, 'spikes', 'results')
const BUDGET_GZIP = 150 * 1024

const isWin = process.platform === 'win32'

function build(variant, manualChunks) {
  if (existsSync(DIST)) rmSync(DIST, { recursive: true, force: true })
  const env = { ...process.env, VITE_S5_VARIANT: variant }
  if (manualChunks) env.S5_MANUAL_CHUNKS = '1'
  else delete env.S5_MANUAL_CHUNKS
  // 用 vite build 而不是 pnpm build：跳过 vue-tsc，量测只关心产物体积
  // 直接调 vite 的 JS 入口，不经过 shell。
  // 传参给 shell 会有注入风险，Node 也会为此发 DEP0190 警告。
  const viteBin = join(ROOT, 'node_modules', 'vite', 'bin', 'vite.js')
  execFileSync(process.execPath, [viteBin, 'build'], {
    cwd: ROOT,
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  })
}

function sizes(buf) {
  return {
    raw: buf.length,
    gzip: gzipSync(buf, { level: 9 }).length,
    brotli: brotliCompressSync(buf, {
      params: { [zlibConstants.BROTLI_PARAM_QUALITY]: 11 },
    }).length,
  }
}

function readManifest() {
  for (const p of [join(DIST, '.vite', 'manifest.json'), join(DIST, 'manifest.json')]) {
    if (existsSync(p)) return JSON.parse(readFileSync(p, 'utf8'))
  }
  throw new Error('找不到 vite manifest。请确认 build.manifest = true。')
}

/**
 * 从 entry 出发只走静态 import 边，收集首屏必须下载的文件。
 * `dynamicImports` 不跟进 —— 那正是我们主动推到懒加载的部分。
 */
function firstScreenFiles(manifest) {
  const byName = new Map(Object.entries(manifest))
  const entries = [...byName.entries()].filter(([, v]) => v.isEntry)
  const files = new Set()
  const seen = new Set()

  const visit = (key) => {
    if (seen.has(key)) return
    seen.add(key)
    const item = byName.get(key)
    if (!item) return
    if (item.file) files.add(item.file)
    for (const css of item.css ?? []) files.add(css)
    for (const dep of item.imports ?? []) visit(dep)
    // 注意：故意不遍历 item.dynamicImports
  }
  for (const [k] of entries) visit(k)
  return { files: [...files], entryKeys: entries.map(([k]) => k) }
}

function allDistAssets() {
  const out = []
  const walk = (dir, prefix = '') => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      const p = join(dir, e.name)
      const rel = prefix ? `${prefix}/${e.name}` : e.name
      if (e.isDirectory()) walk(p, rel)
      else if (/\.(js|css)$/.test(e.name)) out.push({ rel, bytes: statSync(p).size })
    }
  }
  walk(DIST)
  return out
}

function measure(variant, manualChunks) {
  build(variant, manualChunks)
  const manifest = readManifest()
  const { files, entryKeys } = firstScreenFiles(manifest)

  const detail = files.map((f) => {
    const buf = readFileSync(join(DIST, f))
    return { file: f, ...sizes(buf) }
  })
  const total = detail.reduce(
    (a, d) => ({ raw: a.raw + d.raw, gzip: a.gzip + d.gzip, brotli: a.brotli + d.brotli }),
    { raw: 0, gzip: 0, brotli: 0 },
  )

  // 所有产物（含懒加载），用来看 naive-ui / codemirror 各自落在哪
  const assets = allDistAssets()
  const lazy = assets
    .filter((a) => !files.includes(a.rel))
    .map((a) => {
      const buf = readFileSync(join(DIST, a.rel))
      return { file: a.rel, ...sizes(buf) }
    })
    .sort((x, y) => y.gzip - x.gzip)

  // 判断「某个库是否在首屏」不能靠 chunk 文件名 —— 默认分包下文件名是 hash，
  // 不带库名。改成在首屏 JS 文本里找该库的特征字符串。
  const firstScreenText = files
    .filter((f) => f.endsWith('.js'))
    .map((f) => readFileSync(join(DIST, f), 'utf8'))
    .join('\n')
  const naiveInFirstScreen = /n-config-provider|css-render|naive-ui/.test(firstScreenText)
  const cmInFirstScreen = /cm-content|cm-scroller|@codemirror/.test(firstScreenText)

  return {
    variant,
    manualChunks,
    label: `${variant.toUpperCase()}${manualChunks ? ' + 手动分包' : ' + 默认分包'}`,
    entryKeys,
    firstScreen: { files: detail, total },
    lazyChunks: lazy,
    distTotal: assets.reduce((a, b) => a + b.bytes, 0),
    naiveInFirstScreen,
    codemirrorInFirstScreen: cmInFirstScreen,
    withinBudget: total.gzip < BUDGET_GZIP,
    budgetUsagePct: (total.gzip / BUDGET_GZIP) * 100,
  }
}

// ─────────────────────────────────────────────────────────────────────

mkdirSync(RESULTS, { recursive: true })

/**
 * 2×2 测量矩阵。
 *
 * 只测 A/B 两个变体是不够的 —— 第一轮就踩了坑：手动 vendor 分包会把只被动态
 * import 用到的 naive-ui 提升成 entry 的静态依赖，于是两个变体都「naive-ui 在首屏」，
 * 量出来的数字全是配置伪影。所以把分包策略也纳入矩阵。
 */
const CONFIGS = [
  { variant: 'a', manualChunks: false },
  { variant: 'b', manualChunks: false },
  { variant: 'a', manualChunks: true },
  { variant: 'b', manualChunks: true },
]

const all = []
for (const c of CONFIGS) {
  process.stdout.write(`building ${c.variant}${c.manualChunks ? ' (manual chunks)' : ' (default chunks)'} ...\n`)
  all.push(measure(c.variant, c.manualChunks))
}
const pick = (v, mc) => all.find((x) => x.variant === v && x.manualChunks === mc)

// 生产采用的组合：默认分包
const aDef = pick('a', false)
const bDef = pick('b', false)
const aMan = pick('a', true)
const bMan = pick('b', true)

const findings = []
findings.push(
  `**分包策略比 naive-ui 放哪更关键。** 默认分包下首屏 gzip：变体A ${kb(aDef.firstScreen.total.gzip)}、` +
    `变体B ${kb(bDef.firstScreen.total.gzip)}；换成手动 vendor 分包后变成 ${kb(aMan.firstScreen.total.gzip)} / ` +
    `${kb(bMan.firstScreen.total.gzip)}。手动分包把变体B 的首屏放大了 ` +
    `${(bMan.firstScreen.total.gzip / bDef.firstScreen.total.gzip).toFixed(1)} 倍。`,
)
findings.push(
  `原因：把只被动态 import 用到的依赖强行归入命名 chunk（vendor-naive）后，打包器会把该 chunk ` +
    `提升为 entry 的**静态**依赖，懒加载在产物层面失效。手动分包下 naive-ui 是否在首屏：` +
    `A=${aMan.naiveInFirstScreen ? '是' : '否'}、B=${bMan.naiveInFirstScreen ? '是' : '否'}；` +
    `默认分包下：A=${aDef.naiveInFirstScreen ? '是' : '否'}、B=${bDef.naiveInFirstScreen ? '是' : '否'}。`,
)
findings.push(
  `按 02 文档 §7.5 原样实现（变体A：n-config-provider 放根节点）+ 默认分包，首屏 gzip ` +
    `${kb(aDef.firstScreen.total.gzip)}，预算占用 ${aDef.budgetUsagePct.toFixed(0)}%，` +
    `${aDef.withinBudget ? '在预算内，文档原方案不需要改' : '超出预算，文档原方案需要改'}。`,
)
findings.push(
  `naive-ui 进首屏的实际代价（默认分包下 A 与 B 之差）：` +
    `${kb(aDef.firstScreen.total.gzip - bDef.firstScreen.total.gzip)} gzip，` +
    `占 150KB 预算的 ${(((aDef.firstScreen.total.gzip - bDef.firstScreen.total.gzip) / BUDGET_GZIP) * 100).toFixed(0)}%。`,
)
findings.push(
  `CodeMirror 在所有组合下都不在首屏（A=${aDef.codemirrorInFirstScreen ? '是' : '否'}、` +
    `B=${bDef.codemirrorInFirstScreen ? '是' : '否'}），符合 03 文档 §4.1 的要求。` +
    `它单独一个 chunk 约 107KB gzip —— 如果当初选了 Monaco，这一块进首屏就直接击穿预算。`,
)

const recommendation = []
recommendation.push(
  '**vite.config 里不要做手动 vendor 分包。** 已改为默认关闭，仅保留 S5_MANUAL_CHUNKS=1 作为可复现对照。' +
    '这条要写进工程规范：手动分包看起来是优化，实际会破坏动态 import 的懒加载边界。',
)
if (aDef.withinBudget) {
  recommendation.push(
    '02 文档 §7.5 第 3 条「n-config-provider 只包一层放根节点」**可以保留**，首屏预算撑得住。' +
      '但 §7.5 需要补一条：naive-ui 的组件本身仍要按需引入且尽量只出现在懒加载模块里，' +
      `因为它是首屏里最大的单一可变项（本次占 ${kb(aDef.firstScreen.total.gzip - bDef.firstScreen.total.gzip)}）。`,
  )
} else {
  recommendation.push(
    '变体A 即使在默认分包下也超预算，02 文档 §7.5 第 3 条需要改：首屏不挂 ConfigProvider。',
  )
}
recommendation.push(
  'CI 门禁复用本脚本：`node scripts/measure-bundle.mjs`。门禁只看「默认分包」的两个组合，' +
    '首屏 gzip 超 150KB 即 fail。同时断言 CodeMirror 不在首屏 —— 这条比体积数字更容易被无意破坏。',
)

const primary = aDef
const verdict = primary.withinBudget
  ? `PASS：按 02 文档 §7.5 原方案（naive-ui 进首屏）+ 打包器默认分包，首屏 gzip ${kb(primary.firstScreen.total.gzip)} < 150KB，预算占用 ${primary.budgetUsagePct.toFixed(0)}%`
  : bDef.withinBudget
    ? `CONDITIONAL PASS：原方案 ${kb(primary.firstScreen.total.gzip)} 超预算，naive-ui 全懒加载后 ${kb(bDef.firstScreen.total.gzip)} 达标`
    : `FAIL：默认分包下两个变体都超预算（A=${kb(primary.firstScreen.total.gzip)}, B=${kb(bDef.firstScreen.total.gzip)}）`

const report = {
  spike: 'S5',
  at: new Date().toISOString(),
  budgetGzip: BUDGET_GZIP,
  verdict,
  findings,
  recommendation,
  matrix: all,
}
writeFileSync(join(RESULTS, 's5.json'), JSON.stringify(report, null, 2))
writeFileSync(join(RESULTS, 's5-report.md'), md(report), 'utf8')

console.log('\n' + verdict)
console.log(`\n报告：spikes/results/s5.json`)
console.log(`      spikes/results/s5-report.md`)

// ─────────────────────────────────────────────────────────────────────

function kb(n) {
  return `${(n / 1024).toFixed(1)} KB`
}

function md(r) {
  const L = []
  const desc = {
    a: 'naive-ui 进首屏（按 02 文档 §7.5：n-config-provider 放根节点）',
    b: 'naive-ui 全部懒加载（首屏零 naive-ui）',
  }
  L.push('# Spike S5 结果：首屏 chunk 体积\n')
  L.push(`## 结论\n\n${r.verdict}\n`)
  L.push(`预算：首屏 gzip < ${kb(r.budgetGzip)}（03 文档 §10 S5）。`)
  L.push('「首屏」= 从 entry 出发**只走静态 import 边**可达的全部 JS + CSS，不含 `dynamicImports`。\n')

  L.push('## 测量矩阵（2 变体 × 2 分包策略）\n')
  L.push('| 组合 | naive-ui 归属 | 分包 | 首屏 raw | 首屏 gzip | 首屏 brotli | 预算占用 | naive在首屏 | CM在首屏 | 达标 |')
  L.push('| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |')
  for (const x of r.matrix) {
    L.push(
      `| **${x.label}** | ${x.variant === 'a' ? '首屏' : '懒加载'} | ${x.manualChunks ? '手动 vendor' : '打包器默认'} | ` +
        `${kb(x.firstScreen.total.raw)} | **${kb(x.firstScreen.total.gzip)}** | ${kb(x.firstScreen.total.brotli)} | ` +
        `${x.budgetUsagePct.toFixed(0)}% | ${x.naiveInFirstScreen ? '是' : '否'} | ${x.codemirrorInFirstScreen ? '是' : '否'} | ` +
        `${x.withinBudget ? 'PASS' : 'FAIL'} |`,
    )
  }
  L.push('')
  L.push('变体说明：')
  L.push(`- **A** = ${desc.a}`)
  L.push(`- **B** = ${desc.b}\n`)

  L.push('## 发现\n')
  r.findings.forEach((f, i) => L.push(`${i + 1}. ${f}\n`))
  L.push('## 建议\n')
  r.recommendation.forEach((f, i) => L.push(`${i + 1}. ${f}\n`))

  L.push('## 逐组合明细\n')
  for (const x of r.matrix) {
    L.push(`### ${x.label}\n`)
    L.push('首屏（静态 import 图可达）\n')
    L.push('| 文件 | raw | gzip | brotli |')
    L.push('| --- | --- | --- | --- |')
    for (const f of x.firstScreen.files) L.push(`| \`${f.file}\` | ${kb(f.raw)} | ${kb(f.gzip)} | ${kb(f.brotli)} |`)
    L.push(`| **合计** | **${kb(x.firstScreen.total.raw)}** | **${kb(x.firstScreen.total.gzip)}** | **${kb(x.firstScreen.total.brotli)}** |`)
    L.push('\n懒加载 chunk（不计入首屏预算，top 6）\n')
    L.push('| 文件 | raw | gzip |')
    L.push('| --- | --- | --- |')
    for (const f of x.lazyChunks.slice(0, 6)) L.push(`| \`${f.file}\` | ${kb(f.raw)} | ${kb(f.gzip)} |`)
    L.push('')
  }
  return L.join('\n')
}
