#!/usr/bin/env node
/** 把 S2 量测台产出的 s2.json 渲染成可读的 Markdown 报告。 */

import { readFileSync, writeFileSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const RESULTS = join(ROOT, 'spikes', 'results')
const r = JSON.parse(readFileSync(join(RESULTS, 's2.json'), 'utf8'))

const n = (v, d = 1) => (v === null || v === undefined ? '—' : Number(v).toFixed(d))
const mb = (v) => (v === null || v === undefined ? '—' : `${(v / 1048576).toFixed(2)} MB`)
const pct = (v) => `${(v * 100).toFixed(1)}%`

const a10 = r.acceptance['10MB 首屏 < 300ms']
const a100 = r.acceptance['100MB 不崩溃且可预览']
const rows = r.rows
const row1 = rows.find((x) => x.targetBytes === 1024 * 1024)
const row10 = rows.find((x) => x.targetBytes === 10 * 1024 * 1024)
const row100 = rows.find((x) => x.targetBytes === 100 * 1024 * 1024)

// R13 修复前同一台机器、同一合成载荷的基线（2026-07-28）。
const old = {
  oneMbProcessCostMb: 9.03515625,
  oneMbProcessRatio: 9.033502166351376,
  tenMbProcessCostMb: 91.34765625,
  tenMbProcessRatio: 9.133430333261405,
  tenMbIndexMs: 196,
  memAfterAllMb: 105.59375,
}

const rangeTimes = rows.map((x) => x.rangeFetchMs).filter((x) => x > 0)
const rangeMin = Math.min(...rangeTimes)
const rangeMax = Math.max(...rangeTimes)
const r13Pass = row10?.indexMetadataRatio != null && row10.indexMetadataRatio < 3
const pass = a10.pass && a100.pass && r13Pass
const processReduction = 1 - row10.indexMemCostMb / old.tenMbProcessCostMb

const L = []
L.push('# Spike S2 复测：Range 首屏与 SpanIndex\n')
L.push('## 结论\n')
L.push(
  pass
    ? `**PASS。** 10MB 首屏 ${n(a10.measuredMs)}ms（预算 300ms），100MB 首屏 ${n(a100.firstPaintMs)}ms 且进程继续应答；` +
        `R13 已修复：10MB 索引进程增量由 ${n(old.tenMbProcessCostMb, 2)}MB / ${n(old.tenMbProcessRatio, 2)}× ` +
        `降至 ${n(row10.indexMemCostMb, 2)}MB / ${n(row10.indexMemRatio, 2)}×，减少 ${pct(processReduction)}。\n`
    : `**FAIL。** 首屏或 R13 内存验收未通过。10MB=${n(a10.measuredMs)}ms，100MB rendered=${a100.rendered}，` +
        `SpanIndex=${n(row10?.indexMetadataRatio, 2)}×。\n`,
)
L.push(`环境：${r.inTauri ? 'Tauri WebView（WebView2）' : '普通浏览器'}；首屏 Range ${r.firstChunkBytes / 1024}KB；量测时间 ${r.at}。`)
L.push(`跑完全部用例后 Rust 进程私有内存 ${n(r.memAfterAllMb)}MB（修复前 ${n(old.memAfterAllMb)}MB）。\n`)

L.push('## 验收对照\n')
L.push('| 验收项 | 标准 | 实测 | 结果 |')
L.push('| --- | --- | --- | --- |')
L.push(`| 10MB JSON 首屏 | < 300 ms | **${n(a10.measuredMs)} ms** | ${a10.pass ? 'PASS' : 'FAIL'} |`)
L.push(`| 100MB 响应体 | 不崩溃、可预览 | 首屏 ${n(a100.firstPaintMs)} ms，IPC 存活 | ${a100.pass ? 'PASS' : 'FAIL'} |`)
L.push(`| R13 索引元数据 | < 3× body，且不持有 Value/body 副本 | ${mb(row10?.indexBytes)} / **${n(row10?.indexMetadataRatio, 2)}×** | ${r13Pass ? 'PASS' : 'FAIL'} |`)
L.push('')

L.push('## 逐体积明细\n')
L.push('| 体积 | 实际 | 分级 | Range 64KB | **首帧** | DOM | 全量拉取 | 建索引 | 节点数 | SpanIndex 元数据 | 元数据/body | 进程增量 | 树首页 |')
L.push('| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |')
for (const x of rows) {
  L.push(
    `| ${x.size} | ${mb(x.actualBytes)} | ${x.tier} | ${n(x.rangeFetchMs)}ms | **${n(x.firstPaintMs)}ms** | ${x.domNodes} | ` +
      `${x.fullFetchMs == null ? '跳过' : `${n(x.fullFetchMs)}ms`} | ${x.indexMs == null ? '跳过' : `${n(x.indexMs)}ms`} | ` +
      `${x.totalNodes ?? '—'} | ${mb(x.indexBytes)} | ${x.indexMetadataRatio == null ? '—' : `${n(x.indexMetadataRatio, 2)}×`} | ` +
      `${x.indexMemCostMb == null ? '—' : `${n(x.indexMemCostMb, 2)}MB / ${n(x.indexMemRatio, 2)}×`} | ${n(x.treePageMs, 2)}ms |`,
  )
}
L.push('')

L.push('## R13 修复前后\n')
L.push('| 指标 | 修复前 `serde_json::Value` | 修复后 SpanIndex | 变化 |')
L.push('| --- | --- | --- | --- |')
L.push(`| 1MB 索引进程增量 | ${n(old.oneMbProcessCostMb, 2)}MB / ${n(old.oneMbProcessRatio, 2)}× | ${n(row1?.indexMemCostMb, 2)}MB / ${n(row1?.indexMemRatio, 2)}× | -${pct(1 - row1.indexMemCostMb / old.oneMbProcessCostMb)} |`)
L.push(`| 10MB 索引进程增量 | ${n(old.tenMbProcessCostMb, 2)}MB / ${n(old.tenMbProcessRatio, 2)}× | **${n(row10?.indexMemCostMb, 2)}MB / ${n(row10?.indexMemRatio, 2)}×** | **-${pct(processReduction)}** |`)
L.push(`| 10MB 常驻结构 | 完整 Value 树，无独立精确值 | ${mb(row10?.indexBytes)} / ${n(row10?.indexMetadataRatio, 2)}× | body 与索引解耦 |`)
L.push(`| 10MB 构建耗时 | ${n(old.tenMbIndexMs)}ms | ${n(row10?.indexMs)}ms | +${n(row10.indexMs - old.tenMbIndexMs)}ms |`)
L.push(`| 全部用例后私有内存 | ${n(old.memAfterAllMb)}MB | ${n(r.memAfterAllMb)}MB | -${pct(1 - r.memAfterAllMb / old.memAfterAllMb)} |`)
L.push('')

L.push('## 发现\n')
L.push(
  `1. **R13 的 9.1× 膨胀已消除。** SpanIndex 每节点固定 32 字节，只存 parent/children/key-id/kind/value span；` +
    `对象键全局去重，数组下标内联。10MB 合成响应的结构自报 ${mb(row10.indexBytes)}（${n(row10.indexMetadataRatio, 2)}×），` +
    `进程私有内存增量 ${n(row10.indexMemCostMb, 2)}MB（${n(row10.indexMemRatio, 2)}×）。\n`,
)
L.push(
  `2. **完整 body 不再进入 Rust 堆。** Tauri 对发布后不可变的 spool 文件建立只读 mmap，扫描完成立即解除映射；` +
    `树分页只按标量 span 读取最多 4KB。该行为同时由源码结构和 SpanIndex 元数据值验证。\n`,
)
L.push(
  `3. **内存下降换来索引构建变慢。** 10MB 从 ${n(old.tenMbIndexMs)}ms 增至 ${n(row10.indexMs)}ms。` +
    `当前扫描器先完整校验/计数，再按容器建立连续子节点区间，会重复扫描嵌套内容。索引是 2MB 以上显式操作，不影响 ` +
    `${n(a10.measuredMs)}ms 首屏；后续若要优化，应保留 32B 节点布局，改为单次 token 流 + 紧凑重排，而不是恢复 Value。\n`,
)
L.push(
  `4. **Range 首屏仍与 body 体积无关。** 四档耗时 ${rows.map((x) => n(x.rangeFetchMs)).join(' / ')}ms，` +
    `范围 ${n(rangeMin)}–${n(rangeMax)}ms；100MB 仍只读取 64KB。\n`,
)
L.push(
  `5. **LRU 与 Cold 释放已落地。** 索引缓存默认预算 128MB，超限淘汰最久未使用索引；标签转 Cold 可调用 ` +
    `\`drop_index\`，正在分页的 Arc 请求可安全完成。50MB/100MB 继续按 SummaryOnly 产品规则跳过索引。\n`,
)

L.push('## 后续\n')
L.push('1. 保留当前 SummaryOnly 硬阈值；SpanIndex 降低了显式解析代价，但不应因此让 50MB+ 默认建树。')
L.push('2. 将 `indexMetadataRatio < 3` 和 32B 节点布局保留为 R13 回归守门；进程差值只作辅助观测。')
L.push('3. 索引构建耗时可在 v0.2 树视图阶段单独优化，不阻塞当前 S2 首屏验收。')
L.push('')
L.push('---\n')
L.push('原始数据：`spikes/results/s2.json`。修复前基线来自 2026-07-28 同机 S2 结果。')
L.push(`UA：\`${r.userAgent}\``)

writeFileSync(join(RESULTS, 's2-report.md'), L.join('\n'), 'utf8')
console.log(pass ? 'S2 + R13: PASS' : 'S2 + R13: FAIL')
console.log('报告：spikes/results/s2-report.md')
