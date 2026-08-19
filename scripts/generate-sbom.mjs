#!/usr/bin/env node
/**
 * 从锁定的 Cargo 元数据生成 Rust 依赖的 SPDX 2.3 JSON SBOM。
 *
 * 这是「源码/构建输入」SBOM：包含 Cargo workspace 的解析依赖，而不是逐字节描述
 * 某一平台的二进制。前端 production dependencies 由 pnpm 内置的 `pnpm sbom`
 * 另行生成 SPDX 文件；发布工作流会同时提供两份清单、具体安装程序的 SHA-256、
 * 第三方 notices 和签名状态。
 *
 * 用法：node scripts/generate-sbom.mjs [输出文件]
 * 默认输出：apogee-rust.spdx.json（仓库根目录，发布产物，已被 .gitignore 忽略）
 */

import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const OUTPUT = resolve(ROOT, process.argv[2] ?? 'apogee-rust.spdx.json')

function runJson(command, args, label) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  })
  if (result.error) throw new Error(`${label} 无法启动：${result.error.message}`)
  if (result.status !== 0) throw new Error(`${label} 失败（exit=${result.status}）：${result.stderr || result.stdout}`)
  try {
    return JSON.parse(result.stdout)
  } catch (error) {
    throw new Error(`${label} 未返回有效 JSON：${error instanceof Error ? error.message : String(error)}`)
  }
}

function spdxLicense(value) {
  const license = String(value ?? '').trim()
  // Cargo 的许可证元数据通常已经是 SPDX 表达式。对于空值或明显不是 SPDX
  // 表达式的自由文本，使用标准的 NOASSERTION 而不是捏造许可证。
  return license && /^[A-Za-z0-9.+():\- ]+(?:\s+(?:AND|OR|WITH)\s+[A-Za-z0-9.+():\- ]+)*$/.test(license)
    ? license
    : 'NOASSERTION'
}

function createdAt() {
  const epoch = Number(process.env.SOURCE_DATE_EPOCH)
  if (Number.isInteger(epoch) && epoch >= 0) return new Date(epoch * 1000).toISOString()
  return new Date().toISOString()
}

function component({ id, name, version, license, manager, comment }) {
  const expression = spdxLicense(license)
  return {
    SPDXID: id,
    name,
    versionInfo: version,
    downloadLocation: 'NOASSERTION',
    filesAnalyzed: false,
    licenseConcluded: expression,
    licenseDeclared: expression,
    copyrightText: 'NOASSERTION',
    supplier: 'NOASSERTION',
    comment: `Package manager: ${manager}${comment ? `; ${comment}` : ''}`,
  }
}

function rustComponents(metadata) {
  const packages = [...metadata.packages].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
  return packages.map((pkg, index) => component({
    id: `SPDXRef-Cargo-${String(index + 1).padStart(4, '0')}`,
    name: pkg.name,
    version: pkg.version,
    license: pkg.license,
    manager: 'Cargo',
    comment: pkg.repository ? `Repository: ${pkg.repository}` : '',
  }))
}

try {
  if (process.argv.length > 3) throw new Error('用法：node scripts/generate-sbom.mjs [输出文件]')

  const manifest = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8'))
  const cargoMetadata = runJson('cargo', ['metadata', '--format-version', '1', '--locked'], 'cargo metadata')
  const packages = [
    component({
      id: 'SPDXRef-Apogee',
      name: manifest.name ?? 'apogee',
      version: manifest.version ?? 'NOASSERTION',
      license: manifest.license ?? 'MIT',
      manager: 'workspace',
      comment: 'Apogee source project',
    }),
    ...rustComponents(cargoMetadata),
  ]

  const created = createdAt()
  const tag = (process.env.GITHUB_REF_NAME ?? `v${manifest.version ?? 'unknown'}`).replace(/[^A-Za-z0-9._-]/g, '-')
  // GITHUB_SHA 使正式发布的 namespace 与不可变 commit 绑定；本地生成则带时间戳，
  // 满足 SPDX document namespace 在不同生成实例间必须唯一的要求。
  const revision = (process.env.GITHUB_SHA ?? `local-${created.replace(/[:.]/g, '-')}`).replace(/[^A-Za-z0-9._-]/g, '-')
  const document = {
    spdxVersion: 'SPDX-2.3',
    dataLicense: 'CC0-1.0',
    SPDXID: 'SPDXRef-DOCUMENT',
    name: `Apogee-${manifest.version ?? 'unknown'}-rust-source-sbom`,
    documentNamespace: `https://github.com/Leo-Lee-j/apogee/sbom/rust/${tag}/${revision}`,
    creationInfo: {
      created,
      creators: ['Tool: Apogee scripts/generate-sbom.mjs'],
    },
    documentDescribes: ['SPDXRef-Apogee'],
    packages,
    relationships: packages.map((pkg) => ({
      spdxElementId: 'SPDXRef-DOCUMENT',
      relationshipType: 'DESCRIBES',
      relatedSpdxElement: pkg.SPDXID,
    })),
  }

  writeFileSync(OUTPUT, `${JSON.stringify(document, null, 2)}\n`, 'utf8')
  process.stdout.write(`已生成 Rust SPDX 2.3 SBOM：${OUTPUT}（${packages.length} 个包）\n`)
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`)
  process.exitCode = 1
}
