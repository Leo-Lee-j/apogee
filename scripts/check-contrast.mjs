/**
 * 对比度门禁 —— 校验 src/styles/tokens.css 是否满足 02 文档 §7.2 的
 * 「所有正文对比度 ≥ 4.5:1，符合 WCAG 2.1 AA」。
 *
 * 为什么需要这个脚本：
 * 这条声明以前是写在文档里的一句话，没人验过。整合 Apogee 配色时实测发现
 * 规范自称「≥4.5:1 / WCAG AA」的多个值实际不达标（dim 文字 2.35:1、
 * 主按钮白字 2.14:1、焦点环 1.08:1），而项目原有的 --x-text-faint 也只有
 * 2.84:1。配色是「改一个值不会报错」的那类代码，所以留一个可执行的出口。
 *
 * 令牌本身是唯一数据源：脚本直接解析 tokens.css，不复制色值，避免两边漂移。
 *
 * 约定：`--a-{name}` 是前景色，`--a-{name}-bg` 是它的 pill 底色。
 * 底色是 rgba 时按 alpha 合成到 --a-bg-2 上再算。
 *
 * 用法：node scripts/check-contrast.mjs
 * 退出码：0 全部达标；1 存在未达标项。
 */

import { readFile } from 'node:fs/promises'
import { fileURLToPath, URL as NodeURL } from 'node:url'

const TOKENS = fileURLToPath(new NodeURL('../src/styles/tokens.css', import.meta.url))

// ─────────────────────────────────────────────────────────────────────
// 颜色与 WCAG 2.1 相对亮度
// ─────────────────────────────────────────────────────────────────────

/** 解析 #rgb / #rrggbb / rgb() / rgba() → [r,g,b,a]。 */
function parseColor(raw) {
  const s = raw.trim()
  if (s.startsWith('#')) {
    let h = s.slice(1)
    if (h.length === 3) h = h.split('').map((c) => c + c).join('')
    if (h.length !== 6) throw new Error(`无法解析颜色：${raw}`)
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)).concat(1)
  }
  const m = s.match(/^rgba?\(([^)]+)\)$/i)
  if (!m) throw new Error(`无法解析颜色：${raw}`)
  const parts = m[1].split(/[,/]/).map((x) => x.trim())
  const [r, g, b] = parts.slice(0, 3).map(Number)
  const a = parts[3] === undefined ? 1 : Number(parts[3])
  return [r, g, b, a]
}

/** 把带 alpha 的前景合成到不透明底色上。 */
function flatten(fg, bg) {
  const [r, g, b, a] = fg
  if (a >= 1) return [r, g, b, 1]
  return [
    Math.round(a * r + (1 - a) * bg[0]),
    Math.round(a * g + (1 - a) * bg[1]),
    Math.round(a * b + (1 - a) * bg[2]),
    1,
  ]
}

const channel = (c) => {
  const v = c / 255
  return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
}
const luminance = ([r, g, b]) => 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)

function contrast(fg, bg) {
  const l1 = luminance(fg)
  const l2 = luminance(bg)
  const [hi, lo] = l1 > l2 ? [l1, l2] : [l2, l1]
  return (hi + 0.05) / (lo + 0.05)
}

/** 合成结果与底色的最大通道差。用来判断叠加层是否可感知。 */
function channelDelta(fg, bg) {
  const f = flatten(fg, bg)
  return Math.max(...[0, 1, 2].map((i) => Math.abs(f[i] - bg[i])))
}

// ─────────────────────────────────────────────────────────────────────
// 解析 tokens.css
// ─────────────────────────────────────────────────────────────────────

function parseBlock(css, selector) {
  // 调色板块内部没有嵌套花括号，所以 [^}]* 足够。
  // 暗色取**第一个** `:root {`，避免匹配到 @media 里那个只改 --a-dur 的块。
  const re = new RegExp(`${selector}\\s*\\{([^}]*)\\}`)
  const m = css.match(re)
  if (!m) throw new Error(`tokens.css 里找不到 ${selector} 块`)
  const vars = {}
  for (const line of m[1].split('\n')) {
    const d = line.match(/(--[\w-]+)\s*:\s*([^;]+);/)
    if (d) vars[d[1]] = d[2].trim()
  }
  return vars
}

const css = await readFile(TOKENS, 'utf8')
const darkVars = parseBlock(css, ':root')
const lightVars = { ...darkVars, ...parseBlock(css, ":root\\[data-theme='light'\\]") }

// ─────────────────────────────────────────────────────────────────────
// 检查表
// ─────────────────────────────────────────────────────────────────────

/** AA 正文下限。02 文档 §7.2 的硬要求。 */
const AA_TEXT = 4.5
/** AAA 正文。Apogee 规范对主/次文字的自我要求，这两级本来就轻松达标，顺手守住。 */
const AAA_TEXT = 7
/** WCAG 1.4.11 非文字对比（焦点指示器、控件边界）。 */
const NON_TEXT = 3
/** 叠加层可感知阈值。不是 WCAG 条款，是实测下来的经验值：
    8 级通道差在多数显示器上看不出来，12 级以上稳定可见。 */
const OVERLAY_DELTA = 12

const results = []
const push = (theme, group, label, ratio, need, kind = 'ratio') =>
  results.push({ theme, group, label, ratio, need, kind, pass: ratio >= need })

function audit(theme, v) {
  const bgNames = ['--a-bg-1', '--a-bg-2', '--a-bg-3']
  const bgs = bgNames.map((n) => parseColor(v[n]))
  const bg2 = parseColor(v['--a-bg-2'])

  // 文字层级：对三层背景都要达标，取最严格的那层
  const textLevels = [
    ['--a-text', AAA_TEXT],
    ['--a-text-soft', AAA_TEXT],
    ['--a-text-dim', AA_TEXT],
    ['--a-text-faint', AA_TEXT],
  ]
  for (const [name, need] of textLevels) {
    const fg = parseColor(v[name])
    bgs.forEach((bg, i) => {
      push(theme, '文字层级', `${name} on ${bgNames[i]}`, contrast(fg, bg), need)
    })
  }

  // 装饰性令牌：只报告，不设门槛（WCAG 对纯装饰元素豁免）
  for (const name of ['--a-text-ghost', '--a-border', '--a-border-strong', '--a-glow']) {
    push(theme, '装饰（豁免）', `${name} on --a-bg-2`, contrast(flatten(parseColor(v[name]), bg2), bg2), 0, 'info')
  }

  // pill 前景 / 底色成对检查
  for (const bgName of Object.keys(v).filter((k) => k.endsWith('-bg') && !/--a-bg-\d$/.test(k))) {
    const fgName = bgName.replace(/-bg$/, '')
    if (!v[fgName]) throw new Error(`${bgName} 没有对应的前景令牌 ${fgName}`)
    const fg = parseColor(v[fgName])
    const rawBg = parseColor(v[bgName])
    // 1) pill 形态：前景压在合成后的底色上
    push(theme, 'pill（底 + 前景）', `${fgName} on ${bgName}`, contrast(fg, flatten(rawBg, bg2)), AA_TEXT)
    // 2) 裸文字形态：TabBar 把方法名直接渲染成彩色文字，没有 pill 底。
    //    但**实色填充**的徽章（网络错误）不存在裸文字形态 —— 它的前景是接近白的
    //    浅色，只可能压在实色块上。对这类只查 pill 形态。
    if (rawBg[3] < 1) {
      push(theme, 'pill（裸文字）', `${fgName} on --a-bg-2`, contrast(fg, bg2), AA_TEXT)
    }
  }

  // 主色块上的前景（发送按钮）。渐变两端都要各自达标，否则按钮一半文字不可读。
  const onPrimary = parseColor(v['--a-on-primary'])
  for (const name of [
    '--a-primary',
    '--a-primary-hover',
    '--a-primary-pressed',
    '--a-grad-from',
    '--a-grad-to',
  ]) {
    push(theme, '主色块前景', `--a-on-primary on ${name}`, contrast(onPrimary, parseColor(v[name])), AA_TEXT)
  }

  // 主色当文字/图标用（标签栏的未保存圆点、激活指示条）
  push(theme, '主色作前景', '--a-primary on --a-bg-2', contrast(parseColor(v['--a-primary']), bg2), AA_TEXT)

  // 焦点指示器：WCAG 1.4.11，对所有可能的相邻底色都要 ≥3:1
  const focus = parseColor(v['--a-focus'])
  bgs.forEach((bg, i) => {
    push(theme, '焦点指示器', `--a-focus vs ${bgNames[i]}`, contrast(focus, bg), NON_TEXT)
  })

  // 叠加层可感知度
  for (const name of ['--a-hover', '--a-selected']) {
    push(theme, '叠加层可感知', `${name} over --a-bg-2`, channelDelta(parseColor(v[name]), bg2), OVERLAY_DELTA, 'delta')
  }
}

audit('暗', darkVars)
audit('亮', lightVars)

// ─────────────────────────────────────────────────────────────────────
// tokens.css ↔ theme.ts 漂移检查
//
// naive-ui 的主题覆写只能是静态字面量（不能运行时读 CSS 变量，那会把首屏
// 拖进一次同步布局），所以两边是手工对齐的。手工对齐必然会漂移，除非有东西守着。
// ─────────────────────────────────────────────────────────────────────

const THEME = fileURLToPath(new NodeURL('../src/theme.ts', import.meta.url))

/** 抽出 `const <name> = { ... } as const` 里的字符串属性。 */
function parseThemeBlock(src, name) {
  const start = src.indexOf(`const ${name} = {`)
  if (start < 0) throw new Error(`theme.ts 里找不到 ${name}`)
  const end = src.indexOf('} as const', start)
  if (end < 0) throw new Error(`${name} 缺少 "} as const" 结尾`)
  const body = src.slice(start, end)
  const out = {}
  for (const m of body.matchAll(/(\w+)\s*:\s*'([^']+)'/g)) out[m[1]] = m[2]
  return out
}

/** #ABC → #aabbcc，便于逐字比较。 */
function normHex(raw) {
  const s = raw.trim().toLowerCase()
  if (!s.startsWith('#')) return s
  const h = s.slice(1)
  return h.length === 3 ? `#${h.split('').map((c) => c + c).join('')}` : `#${h}`
}

/** naive-ui 键 → tokens.css 变量。popoverColor 刻意不列：两个主题的层级取值不同。 */
const THEME_MAP = {
  primaryColor: '--a-primary',
  primaryColorHover: '--a-primary-hover',
  primaryColorPressed: '--a-primary-pressed',
  textColorBase: '--a-text',
  textColor1: '--a-text',
  textColor2: '--a-text-soft',
  textColor3: '--a-text-dim',
  bodyColor: '--a-bg-1',
  cardColor: '--a-bg-2',
  modalColor: '--a-bg-2',
  inputColor: '--a-bg-3',
  tableColor: '--a-bg-2',
  borderColor: '--a-border',
  dividerColor: '--a-border',
}

const themeSrc = await readFile(THEME, 'utf8')
const themeDark = parseThemeBlock(themeSrc, 'commonDark')
// commonLight 先展开 commonDark 再覆盖，这里复现同样的语义
const themeLight = { ...themeDark, ...parseThemeBlock(themeSrc, 'commonLight') }

const drift = []
for (const [theme, themeVars, cssVars] of [
  ['暗', themeDark, darkVars],
  ['亮', themeLight, lightVars],
]) {
  for (const [key, cssVar] of Object.entries(THEME_MAP)) {
    const got = themeVars[key]
    const want = cssVars[cssVar]
    if (got === undefined) {
      drift.push(`${theme} · theme.ts 缺少 ${key}`)
      continue
    }
    if (normHex(got) !== normHex(want)) {
      drift.push(`${theme} · ${key} = ${got}，但 tokens.css 的 ${cssVar} = ${want}`)
    }
  }
}

// ─────────────────────────────────────────────────────────────────────
// 输出
// ─────────────────────────────────────────────────────────────────────

let last = ''
for (const r of results) {
  const key = `${r.theme} · ${r.group}`
  if (key !== last) {
    last = key
    process.stdout.write(`\n=== ${key} ===\n`)
  }
  const value = r.kind === 'delta' ? `${String(r.ratio).padStart(5)} 级` : `${r.ratio.toFixed(2).padStart(6)}:1`
  const need = r.kind === 'info' ? '  —' : r.kind === 'delta' ? `≥${r.need} 级` : `≥${r.need}`
  const verdict = r.kind === 'info' ? 'INFO' : r.pass ? 'PASS' : 'FAIL'
  const mark = verdict === 'FAIL' ? '!' : ' '
  process.stdout.write(`${mark} ${r.label.padEnd(42)} ${value}  ${need.padStart(8)}  ${verdict}\n`)
}

const checked = results.filter((r) => r.kind !== 'info')
const failed = checked.filter((r) => !r.pass)
process.stdout.write(`\n共检查 ${checked.length} 项（另有 ${results.length - checked.length} 项装饰豁免），FAIL ${failed.length} 项\n`)
process.stdout.write(`tokens.css ↔ theme.ts 漂移：${drift.length === 0 ? '无' : `${drift.length} 处`}\n`)

if (failed.length) {
  process.stdout.write('\n对比度未达标：\n')
  for (const f of failed) process.stdout.write(`  ${f.theme} · ${f.label}\n`)
}
if (drift.length) {
  process.stdout.write('\n色值漂移：\n')
  for (const d of drift) process.stdout.write(`  ${d}\n`)
}
if (failed.length || drift.length) process.exit(1)
process.stdout.write('对比度门禁通过。\n')
