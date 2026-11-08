import { ipc, type Body, type RequestSpec } from '@/ipc'

export interface CurlImportResult {
  spec: RequestSpec
  warnings: string[]
}

export async function importCurl(command: string): Promise<CurlImportResult> {
  return ipc.curlImport(command.trim())
}

function quote(value: string): string {
  return `'${value.replaceAll("'", `'"'"'`)}'`
}

export function exportCurl(spec: RequestSpec): string {
  const parts = ['curl', '--request', quote(spec.method || 'GET')]
  const auth = spec.auth
  const explicitAuthorization = (spec.headers ?? []).some(h => h.enabled && h.key.trim().toLowerCase() === 'authorization')
  if (!explicitAuthorization && (auth?.type === 'oauth1' || auth?.type === 'jwt')) {
    throw new Error('OAuth 1.0 / JWT 需要发送时签名；请导出 Postman 集合以保留鉴权配置。')
  }
  if (!explicitAuthorization && auth?.type === 'basic') parts.push('--user', quote(`${auth.username}:${auth.password}`))
  else if (!explicitAuthorization && auth?.type === 'bearer') parts.push('--header', quote(`Authorization: Bearer ${auth.token}`))
  else if (!explicitAuthorization && auth?.type === 'oauth2') parts.push('--header', quote(`Authorization: ${auth.headerPrefix ? `${auth.headerPrefix} ` : ''}${auth.accessToken}`))
  else if (auth?.type === 'apiKey' && auth.location === 'header') parts.push('--header', quote(`${auth.key}: ${auth.value}`))

  for (const header of spec.headers ?? []) {
    if (header.enabled && header.key.trim()) parts.push('--header', quote(`${header.key}: ${header.value}`))
  }
  if (auth?.type === 'apiKey' && auth.location === 'cookie' && !(spec.headers ?? []).some(h => h.enabled && h.key.trim().toLowerCase() === 'cookie')) parts.push('--header', quote(`Cookie: ${auth.key}=${auth.value}`))
  appendBody(parts, spec.body ?? { type: 'none' })
  if (spec.settings?.verifyTls === false) parts.push('--insecure')
  if (spec.settings?.followRedirects) parts.push('--location')
  if (spec.settings?.timeoutMs) parts.push('--max-time', String(spec.settings.timeoutMs / 1000))
  parts.push(quote(buildExportUrl(spec)))
  return parts.join(' \\\n  ')
}

function appendBody(parts: string[], body: Body) {
  if (body.type === 'json') parts.push('--header', quote('Content-Type: application/json'), '--data-raw', quote(body.content))
  else if (body.type === 'text') parts.push('--header', quote(`Content-Type: ${body.contentType}`), '--data-raw', quote(body.content))
  else if (body.type === 'binary') parts.push('--data-binary', quote(`@${body.path}`))
  else if (body.type === 'urlEncoded') {
    for (const item of body.rows.filter((item) => item.enabled && item.key)) parts.push('--data-urlencode', quote(`${item.key}=${item.value}`))
  } else if (body.type === 'formData') {
    for (const item of body.rows.filter((item) => item.enabled && item.key)) parts.push('--form', quote(`${item.key}=${item.kind === 'file' ? '@' : ''}${item.value}`))
  } else if (body.type === 'graphQl') {
    const payload: Record<string, unknown> = { query: body.query, variables: JSON.parse(body.variables || '{}') }
    if (body.operationName) payload.operationName = body.operationName
    parts.push('--header', quote('Content-Type: application/json'), '--data-raw', quote(JSON.stringify(payload)))
  }
}

function buildExportUrl(spec: RequestSpec): string {
  let url = spec.url
  for (const item of spec.pathParams ?? []) {
    if (!item.enabled || !item.key) continue
    const value = encodeURIComponent(item.value)
    url = url.replaceAll(`:${item.key}`, value).replaceAll(`{${item.key}}`, value)
  }
  const query = (spec.params ?? []).filter((item) => item.enabled && item.key)
    .map((item) => `${encodeURIComponent(item.key)}=${encodeURIComponent(item.value)}`)
  const auth = spec.auth
  if (auth?.type === 'apiKey' && auth.location === 'query') query.push(`${encodeURIComponent(auth.key)}=${encodeURIComponent(auth.value)}`)
  if (query.length) url += `${url.includes('?') ? '&' : '?'}${query.join('&')}`
  return url
}
