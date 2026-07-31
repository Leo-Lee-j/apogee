#!/usr/bin/env node
/**
 * 生成占位应用图标。
 *
 * 为什么用脚本生成而不是塞一个二进制文件进仓库：
 *   - 图标是构建的硬依赖（tauri-build 缺 icon.ico 直接失败），必须可复现
 *   - 占位资源被误当成正式设计稿的风险很高，脚本生成能明确它是临时的
 *
 * 正式视觉稿到位后删掉这个脚本，换成设计交付的图标集。
 *
 * 用法：node scripts/gen-icons.mjs
 */

import { writeFileSync, mkdirSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { deflateSync } from 'node:zlib'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const ICONS = join(ROOT, 'src-tauri', 'icons')
mkdirSync(ICONS, { recursive: true })

// 与 src/styles/tokens.css 对齐：--a-primary #4F80FF、--a-bg-1 #0C0E12（02 文档 §7.2）
const BRAND = [0x4f, 0x80, 0xff]
const DARK = [0x0c, 0x0e, 0x12]

/** 画一个圆角方块底 + 中间一条斜杠，纯占位。返回 RGBA 像素。 */
function render(size) {
  const px = Buffer.alloc(size * size * 4)
  const r = size * 0.22
  const inset = size * 0.06
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4
      const inRounded = roundedRect(x, y, inset, inset, size - inset * 2, size - inset * 2, r)
      if (!inRounded) {
        px[i + 3] = 0
        continue
      }
      // 中间一条 45° 斜杠，做出 "/" 的感觉
      const t = (x - y) / size
      const isSlash = Math.abs(t) < 0.09
      const c = isSlash ? DARK : BRAND
      px[i] = c[0]
      px[i + 1] = c[1]
      px[i + 2] = c[2]
      px[i + 3] = 255
    }
  }
  return px
}

function roundedRect(x, y, rx, ry, w, h, r) {
  if (x < rx || y < ry || x > rx + w || y > ry + h) return false
  const cx = Math.min(Math.max(x, rx + r), rx + w - r)
  const cy = Math.min(Math.max(y, ry + r), ry + h - r)
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r + r
}

// ── PNG 编码 ────────────────────────────────────────────────────────
function crc32(buf) {
  let c = ~0
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i]
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1))
  }
  return ~c >>> 0
}

function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(td))
  return Buffer.concat([len, td, crc])
}

function png(size, rgba) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // RGBA
  const raw = Buffer.alloc((size * 4 + 1) * size)
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0 // filter: none
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

// ── ICO 编码（PNG 载荷，Vista+ 支持）────────────────────────────────
function ico(entries) {
  const dir = Buffer.alloc(6 + 16 * entries.length)
  dir.writeUInt16LE(0, 0)
  dir.writeUInt16LE(1, 2) // type = icon
  dir.writeUInt16LE(entries.length, 4)
  let offset = dir.length
  const blobs = []
  entries.forEach((e, i) => {
    const o = 6 + 16 * i
    dir[o] = e.size >= 256 ? 0 : e.size
    dir[o + 1] = e.size >= 256 ? 0 : e.size
    dir[o + 2] = 0
    dir[o + 3] = 0
    dir.writeUInt16LE(1, o + 4)
    dir.writeUInt16LE(32, o + 6)
    dir.writeUInt32LE(e.data.length, o + 8)
    dir.writeUInt32LE(offset, o + 12)
    offset += e.data.length
    blobs.push(e.data)
  })
  return Buffer.concat([dir, ...blobs])
}

const SIZES = [16, 32, 48, 128, 256]
const pngs = SIZES.map((s) => ({ size: s, data: png(s, render(s)) }))

writeFileSync(join(ICONS, 'icon.ico'), ico(pngs))
for (const p of pngs) writeFileSync(join(ICONS, `${p.size}x${p.size}.png`), p.data)
writeFileSync(join(ICONS, 'icon.png'), png(512, render(512)))

console.log('生成占位图标：')
console.log('  src-tauri/icons/icon.ico  （多尺寸：' + SIZES.join(', ') + '）')
console.log('  src-tauri/icons/icon.png  （512）')
console.log('  src-tauri/icons/<size>x<size>.png')
console.log('注意：这是占位资源，正式设计稿到位后替换并删除本脚本。')
