import type { KeyValue, RequestSpec } from '@/ipc'
import { postmanAuth } from './requestAuth'

function rows(values: KeyValue[] = [], form = false) {
  return values.map(row => ({ key: row.key, disabled: !row.enabled,
    ...(form && row.kind === 'file' ? { type: 'file', src: row.value } : { value: row.value, ...(form ? { type: 'text' } : {}) }),
    ...(row.description ? { description: row.description } : {}),
  }))
}
function body(value: RequestSpec['body']): Record<string, unknown> | undefined {
  if (!value || value.type === 'none') return undefined
  if (value.type === 'json' || value.type === 'text') return { mode: 'raw', raw: value.content, options: { raw: { language: value.type === 'json' ? 'json' : 'text' } } }
  if (value.type === 'formData') return { mode: 'formdata', formdata: rows(value.rows, true) }
  if (value.type === 'urlEncoded') return { mode: 'urlencoded', urlencoded: rows(value.rows) }
  if (value.type === 'binary') return { mode: 'file', file: { src: value.path } }
  return { mode: 'graphql', graphql: { query: value.query, variables: value.variables, ...(value.operationName ? { operationName: value.operationName } : {}) } }
}

export function postmanRequest(spec: RequestSpec): Record<string, unknown> {
  const content = body(spec.body), auth = postmanAuth(spec.auth)
  const headers = rows(spec.headers)
  if (spec.body?.type === 'text' && !spec.headers?.some(row => row.enabled && row.key.toLowerCase() === 'content-type')) {
    headers.push({ key: 'Content-Type', value: spec.body.contentType, disabled: false })
  }
  // Convert only the path portion; credentials, query and fragments may contain literal tokens.
  const end = spec.url.search(/[?#]/)
  const address = end < 0 ? spec.url : spec.url.slice(0, end)
  const suffix = end < 0 ? '' : spec.url.slice(end)
  const authority = /^[A-Za-z][A-Za-z0-9+.-]*:[/][/]/.exec(address)?.[0].length ?? 0
  const start = address.startsWith('/') && !authority ? 0 : address.indexOf('/', authority)
  let raw = spec.url
  if (start >= 0) {
    const names = new Set(spec.pathParams?.map(row => row.key))
    const path = address.slice(start).replace(/{{([A-Za-z_][A-Za-z0-9_-]*)}}|{([A-Za-z_][A-Za-z0-9_-]*)}/g, (token, double: string, single: string) => names.has(double ?? single) ? `:${double ?? single}` : token)
    raw = address.slice(0, start) + path + suffix
  }
  raw = raw.replace(/{{env:([^{}]+)}}/g, '{{$1}}')
  return { method: spec.method, header: headers, url: { raw,
    ...(spec.params?.length ? { query: rows(spec.params) } : {}),
    ...(spec.pathParams?.length ? { variable: rows(spec.pathParams) } : {}),
  }, ...(content ? { body: content } : {}), ...(auth ? { auth } : {}) }
}
