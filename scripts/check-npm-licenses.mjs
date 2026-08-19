#!/usr/bin/env node
/**
 * 前端生产依赖的许可证门禁与归属声明生成器。
 *
 * pnpm 自带的 `licenses list --json --prod` 是锁文件解析结果；我们直接消费它，
 * 不再额外引入许可证扫描器或在脚本中复制依赖图。
 *
 * 用法：
 *   node scripts/check-npm-licenses.mjs
 *   node scripts/check-npm-licenses.mjs --notices
 *
 * 默认模式只验证许可证；--notices 还生成根目录的
 * THIRD-PARTY-NOTICES-NPM.txt。该文件是发布产物，保持 .gitignore，避免每次
 * 依赖变化造成工作树噪声。
 */

import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const OUTPUT = join(ROOT, 'THIRD-PARTY-NOTICES-NPM.txt')
const PNPM = 'pnpm'

// 这组表达式与项目当前可接受的宽松许可证策略对齐。新增表达式必须经过
// 许可证评估，而不是为了让 CI 变绿而随意放宽。
const ALLOWED_LICENSES = new Set([
  'MIT',
  'MIT-0',
  'Apache-2.0',
  'Apache-2.0 OR MIT',
  'MIT OR Apache-2.0',
  'BSD-2-Clause',
  'BSD-3-Clause',
  'ISC',
  'Zlib',
  'Unlicense',
  'CC0-1.0',
  '0BSD',
])

function fail(message) {
  process.stderr.write(`${message}\n`)
  process.exitCode = 1
}

function pnpmLicenseInventory() {
  const options = {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  }
  // Windows 不能直接 CreateProcess .cmd。这里的 pnpm 子命令和参数完全固定，
  // 因此通过 cmd.exe 调用时没有用户输入可被解释；其他平台仍直接执行 pnpm。
  const result = process.platform === 'win32'
    ? spawnSync(process.env.ComSpec ?? 'cmd.exe', ['/d', '/s', '/c', 'pnpm licenses list --json --prod'], options)
    : spawnSync(PNPM, ['licenses', 'list', '--json', '--prod'], options)
  if (result.error) throw new Error(`无法执行 pnpm licenses list：${result.error.message}`)
  if (result.status !== 0) {
    throw new Error(`pnpm licenses list 失败（exit=${result.status}）：${result.stderr || result.stdout}`)
  }
  try {
    return JSON.parse(result.stdout)
  } catch (error) {
    throw new Error(`pnpm licenses list 未返回有效 JSON：${error instanceof Error ? error.message : String(error)}`)
  }
}

function recordsFromInventory(inventory) {
  const records = []
  for (const [groupLicense, entries] of Object.entries(inventory)) {
    if (!Array.isArray(entries)) throw new Error(`许可证 ${groupLicense} 的清单格式无效`)
    for (const entry of entries) {
      const license = String(entry.license ?? groupLicense).trim()
      const versions = [...new Set((entry.versions ?? []).map(String))].sort()
      const paths = [...new Set((entry.paths ?? []).map(String))].sort()
      if (!entry.name || versions.length === 0 || paths.length === 0) {
        throw new Error(`许可证清单缺少包名、版本或路径：${JSON.stringify(entry)}`)
      }
      records.push({
        name: String(entry.name),
        versions,
        paths,
        license,
        author: entry.author ? String(entry.author) : 'NOASSERTION',
        homepage: entry.homepage ? String(entry.homepage) : '',
      })
    }
  }
  return records.sort((a, b) => a.name.localeCompare(b.name) || a.versions.join(',').localeCompare(b.versions.join(',')))
}

function assertAllowed(records) {
  const rejected = records.filter((entry) => !ALLOWED_LICENSES.has(entry.license))
  if (rejected.length === 0) return
  const detail = rejected.map((entry) => `- ${entry.name}@${entry.versions.join(', ')}: ${entry.license || '(missing)'}`).join('\n')
  throw new Error(
    `发现不在前端生产依赖许可证白名单内的包：\n${detail}\n` +
    '请评估许可证义务后显式更新本脚本和项目政策；不要通过删除检查绕过。',
  )
}

function noticeFiles(packagePath) {
  if (!existsSync(packagePath)) throw new Error(`依赖目录不存在：${packagePath}`)
  return readdirSync(packagePath)
    .filter((name) => /^(licen[cs]e|copying|notice)(?:[._-].*)?$/i.test(name))
    .sort((a, b) => a.localeCompare(b))
    .map((name) => join(packagePath, name))
}

function mitFallback(author) {
  // npm 包偶尔只发布 license 字段、而没有把标准 MIT 正文放进 tarball。对于
  // 这种狭窄情形，复用仓库 MIT 的标准正文并替换版权行；非 MIT 的缺失文本仍失败，
  // 防止以不正确的通用模板掩盖再分发义务。
  const mit = readFileSync(join(ROOT, 'LICENSE'), 'utf8')
  if (!mit.startsWith('MIT License\n')) throw new Error('根目录 LICENSE 不再是可用于 MIT 回退的标准文本')
  return mit.replace(/^Copyright \(c\) .+$/m, `Copyright (c) ${author}`)
}

function renderNotices(records) {
  const sections = [
    'Apogee npm Third-Party Software Notices',
    '=========================================',
    '',
    'This file is generated from pnpm production dependency metadata and bundled license files.',
    'When a package declares MIT but omits its license file, the standard MIT text is reproduced',
    'with the package metadata author (or NOASSERTION) recorded below. Other missing license texts fail generation.',
    'Do not edit by hand. Regenerate it with: pnpm licenses:npm:notices',
    '',
  ]
  const mitFallbacks = []

  for (const entry of records) {
    const files = noticeFiles(entry.paths[0])
    sections.push('----------------------------------------------------------------------')
    sections.push(`${entry.name} ${entry.versions.join(', ')}`)
    sections.push(`Declared license: ${entry.license}`)
    sections.push(`Package metadata author: ${entry.author}`)
    if (entry.homepage) sections.push(`Homepage: ${entry.homepage}`)
    sections.push('----------------------------------------------------------------------')

    if (files.length === 0) {
      if (entry.license !== 'MIT') {
        throw new Error(
          `${entry.name}@${entry.versions.join(', ')} 没有可识别的 LICENSE / NOTICE 文件，` +
          `且声明的 ${entry.license} 不允许使用 MIT 回退；不能在缺少归属材料时继续发布。`,
        )
      }
      mitFallbacks.push(`${entry.name}@${entry.versions.join(', ')}`)
      sections.push('\n[MIT license text reconstructed because the published package omitted a license file]\n')
      sections.push(mitFallback(entry.author).trimEnd())
      sections.push('')
      continue
    }

    for (const file of files) {
      sections.push(`\n[${file.split(/[\\/]/).at(-1)}]\n`)
      sections.push(readFileSync(file, 'utf8').trimEnd())
      sections.push('')
    }
  }

  sections.push('----------------------------------------------------------------------')
  sections.push('End of notices')
  sections.push('')
  return { text: sections.join('\n'), mitFallbacks }
}

const args = new Set(process.argv.slice(2))
if ([...args].some((arg) => arg !== '--notices')) {
  fail('用法：node scripts/check-npm-licenses.mjs [--notices]')
} else {
  try {
    const records = recordsFromInventory(pnpmLicenseInventory())
    assertAllowed(records)
    const uniqueLicenses = new Set(records.map((entry) => entry.license))
    process.stdout.write(`npm 生产依赖许可证检查通过：${records.length} 个包，${uniqueLicenses.size} 种许可证表达式。\n`)

    if (args.has('--notices')) {
      const notices = renderNotices(records)
      writeFileSync(OUTPUT, notices.text, 'utf8')
      process.stdout.write(`已生成 ${OUTPUT}\n`)
      if (notices.mitFallbacks.length > 0) {
        process.stderr.write(`提示：${notices.mitFallbacks.length} 个 MIT 包未在发布包中携带许可证文件，已记录受控 MIT 回退：${notices.mitFallbacks.join(', ')}\n`)
      }
    }
  } catch (error) {
    fail(error instanceof Error ? error.message : String(error))
  }
}
