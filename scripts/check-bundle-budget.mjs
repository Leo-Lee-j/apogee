#!/usr/bin/env node
// Gate the actual production entry's static JS/CSS graph. Dynamic features stay separate.
import { readFileSync } from 'node:fs'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gzipSync } from 'node:zlib'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const dist = resolve(root, 'dist')
const manifest = JSON.parse(readFileSync(resolve(dist, '.vite/manifest.json'), 'utf8'))
const budget = Number(process.env.APOGEE_BUNDLE_BUDGET_BYTES ?? 150 * 1024)
if (!Number.isSafeInteger(budget) || budget < 1) throw new Error('Invalid gzip budget')
const visited = new Set(), files = new Set()
function visit(key) {
  if (visited.has(key)) return
  visited.add(key)
  const chunk = manifest[key]
  if (!chunk) throw new Error(`Missing manifest dependency: ${key}`)
  if (/[.](js|css)$/.test(chunk.file)) files.add(chunk.file)
  for (const css of chunk.css ?? []) files.add(css)
  for (const dependency of chunk.imports ?? []) visit(dependency)
}
const entries = Object.keys(manifest).filter(key => manifest[key].isEntry)
if (!entries.length) throw new Error('No production entry in manifest')
for (const entry of entries) visit(entry)
const details = [...files].map(file => ({ file, gzipBytes: gzipSync(readFileSync(resolve(dist, file)), { level: 9 }).length }))
const bytes = details.reduce((sum, item) => sum + item.gzipBytes, 0)
for (const item of details.sort((a, b) => b.gzipBytes - a.gzipBytes)) console.log(`${String(item.gzipBytes).padStart(7)}  ${item.file}`)
console.log(`Initial static JS/CSS gzip: ${bytes} bytes (${(bytes / 1024).toFixed(2)} KiB), budget ${budget} bytes (${(budget / 1024).toFixed(2)} KiB): ${bytes <= budget ? 'PASS' : 'FAIL'}`)
if (bytes > budget) process.exitCode = 1
