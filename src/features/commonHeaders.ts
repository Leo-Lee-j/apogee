import { version as appVersion } from '../../package.json'
import type { KeyValue } from '@/ipc'

/**
 * 浏览器、HTTP 客户端或协议层惯例生成的 Header。导入 cURL / HAR 时这些 Header
 * 往往被解析为普通行，但默认视图仍应隐藏它们，只聚焦业务自定义 Header。
 */
const OFFICIAL_HEADER_NAMES = new Set([
  'accept',
  'accept-charset',
  'accept-encoding',
  'accept-language',
  'access-control-request-headers',
  'access-control-request-method',
  'baggage',
  'cache-control',
  'connection',
  'content-length',
  'content-type',
  'cookie',
  'device-memory',
  'dnt',
  'downlink',
  'dpr',
  'ect',
  'expect',
  'host',
  'if-match',
  'if-modified-since',
  'if-none-match',
  'if-range',
  'if-unmodified-since',
  'origin',
  'pragma',
  'priority',
  'purpose',
  'range',
  'referer',
  'requestid',
  'rtt',
  'save-data',
  'sec-gpc',
  'sentry-trace',
  'service-worker-navigation-preload',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
  'upgrade-insecure-requests',
  'user-agent',
  'via',
  'viewport-width',
  'warning',
  'width',
  'x-client-data',
])

const OFFICIAL_HEADER_PREFIXES = ['sec-ch-', 'sec-fetch-']

/** 判断名称是否属于已知浏览器/协议通用 Header。 */
export function isKnownOfficialHeader(name: string): boolean {
  const normalized = name.trim().toLowerCase()
  return OFFICIAL_HEADER_NAMES.has(normalized)
    || OFFICIAL_HEADER_PREFIXES.some((prefix) => normalized.startsWith(prefix))
}

/**
 * 这里仅保留发送前可准确确定值的通用 Header。Host、Content-Length、
 * Content-Type 等协议/请求体 Header 在实际请求完成组装后才生成，
 * 会由响应区的实际请求表格通过 commonHeaderNames 一并显示或隐藏。
 */
const PREVIEWABLE_OFFICIAL_HEADERS = [
  { key: 'Accept', value: '*/*' },
  { key: 'User-Agent', value: `Apogee/${appVersion}` },
] as const

/** 返回当前请求未显式覆盖的、可在发送前预览的通用 Header。 */
export function commonHeadersFor(headers: KeyValue[]): KeyValue[] {
  const explicitNames = new Set(
    headers
      .filter((header) => header.enabled && header.key.trim())
      .map((header) => header.key.trim().toLowerCase()),
  )

  return PREVIEWABLE_OFFICIAL_HEADERS
    .filter((header) => !explicitNames.has(header.key.toLowerCase()))
    .map((header) => ({
      enabled: true,
      key: header.key,
      value: header.value,
      description: '由 Apogee 自动附加',
      kind: 'text' as const,
    }))
}
