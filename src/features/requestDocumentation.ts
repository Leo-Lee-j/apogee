import type { AuthConfig, Body, KeyValue, RequestSettings } from '@/ipc'
import type { Tab } from '@/stores/tabs'

export const NO_PARAMETER_DESCRIPTION = '未提供参数说明'

export interface RequestDocRow {
  name: string
  type: string
  description: string
  status: string
}

export interface RequestDocTreeRow extends RequestDocRow {
  depth: number
  hasChildren: boolean
}

export interface RequestDocSection {
  id: string
  title: string
  kind: 'table' | 'tree' | 'summary'
  count?: number
  rows?: RequestDocRow[]
  tree?: RequestDocTreeRow[]
  value?: string
  detail?: string
}

export interface RequestDocViewModel {
  title: string
  method: string
  url: string
  copyUrl: string
  dirty: boolean
  isEmpty: boolean
  sections: RequestDocSection[]
}

const SENSITIVE_NAME_PATTERN = /(?:authorization|proxy[-_]?authorization|cookie|set[-_]?cookie|x[-_]?api[-_]?key|api[-_]?key|access[-_]?token|refresh[-_]?token|token|secret|password)/i

/**
 * Names are deliberately checked in every projection path. A user can mark a
 * value as ordinary text while its name still makes it a credential.
 */
export function isSensitiveName(name: string): boolean {
  return SENSITIVE_NAME_PATTERN.test(name.trim())
}

function decodeQueryName(value: string): string {
  try {
    return decodeURIComponent(value.replaceAll('+', ' '))
  } catch {
    return value
  }
}

function normalizedName(name: string): string {
  return name.trim().replaceAll('-', '').replaceAll('_', '').toLowerCase()
}

function isTemplateVariable(value: string): boolean {
  return /^\{\{[A-Za-z_][A-Za-z0-9_-]*\}\}$/.test(value.trim())
}

function isPathPlaceholder(value: string): boolean {
  const trimmed = value.trim()
  return isTemplateVariable(trimmed)
    || /^:[A-Za-z_][A-Za-z0-9_-]*$/.test(trimmed)
    || /^\{[A-Za-z_][A-Za-z0-9_-]*\}$/.test(trimmed)
}

function decodePathSegment(value: string): string {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

/**
 * Replace path parameter values only when the value occupies a complete path
 * segment. This keeps ordinary endpoint segments while preventing a literal
 * path id from becoming part of the documentation snapshot.
 */
function sanitizePathTemplate(
  url: string,
  pathParams: Iterable<Pick<KeyValue, 'key' | 'value'>>,
): string {
  const candidates = [...pathParams]
    .map((item) => ({ key: item.key.trim(), value: item.value.trim() }))
    .filter((item) => item.key && item.value && !isPathPlaceholder(item.value))
  if (!candidates.length) return url

  const authorityStart = url.indexOf('//')
  let pathStart = 0
  if (authorityStart >= 0) {
    const authorityContentStart = authorityStart + 2
    const authorityEndOffset = url.slice(authorityContentStart).search(/[/?#]/)
    pathStart = authorityEndOffset < 0 ? url.length : authorityContentStart + authorityEndOffset
  }
  const pathEndOffset = url.slice(pathStart).search(/[?#]/)
  const pathEnd = pathEndOffset < 0 ? url.length : pathStart + pathEndOffset
  const pathname = url.slice(pathStart, pathEnd)
  const sanitizedPath = pathname.split('/').map((segment) => {
    const decodedSegment = decodePathSegment(segment)
    const match = candidates.find((candidate) => {
      const encodedValue = encodeURIComponent(candidate.value)
      return segment === candidate.value || segment === encodedValue || decodedSegment === candidate.value
    })
    return match ? `{${match.key}}` : segment
  }).join('/')
  return `${url.slice(0, pathStart)}${sanitizedPath}${url.slice(pathEnd)}`
}

/**
 * Produce an endpoint template rather than a request snapshot:
 * - URL userinfo is removed;
 * - query values are omitted;
 * - variable placeholders remain useful to developers unless the parameter is
 *   explicitly sensitive.
 */
export function sanitizeUrlTemplate(
  url: string,
  secretQueryNames: Iterable<string> = [],
  pathParams: Iterable<Pick<KeyValue, 'key' | 'value'>> = [],
): string {
  const authorityStart = url.indexOf('//')
  let sanitizedUrl = url
  if (authorityStart >= 0) {
    const authorityContentStart = authorityStart + 2
    const authorityEndOffset = url.slice(authorityContentStart).search(/[/?#]/)
    const authorityEnd = authorityEndOffset < 0 ? url.length : authorityContentStart + authorityEndOffset
    const authority = url.slice(authorityContentStart, authorityEnd)
    const userInfoEnd = authority.lastIndexOf('@')
    if (userInfoEnd >= 0) {
      // URL userinfo is credential material, not endpoint documentation.
      sanitizedUrl = `${url.slice(0, authorityContentStart)}${authority.slice(userInfoEnd + 1)}${url.slice(authorityEnd)}`
    }
  }

  sanitizedUrl = sanitizePathTemplate(sanitizedUrl, pathParams)

  const sensitiveNames = new Set([...secretQueryNames].map(normalizedName))
  const queryStart = sanitizedUrl.indexOf('?')
  const hashStart = sanitizedUrl.indexOf('#', queryStart < 0 ? 0 : queryStart)
  const queryEnd = hashStart < 0 ? sanitizedUrl.length : hashStart
  if (queryStart < 0) return sanitizedUrl.slice(0, queryEnd)

  const query = sanitizedUrl.slice(queryStart + 1, queryEnd)
  const params = query.split('&').filter(Boolean).map((part) => {
    const separator = part.indexOf('=')
    if (separator < 0) return part

    const name = part.slice(0, separator)
    const decodedName = decodeQueryName(name)
    const value = part.slice(separator + 1)
    const keepPlaceholder = isTemplateVariable(value)
      && !isSensitiveName(decodedName)
      && !sensitiveNames.has(normalizedName(decodedName))
    return keepPlaceholder ? `${name}=${value}` : name
  })
  const queryTemplate = params.length ? `?${params.join('&')}` : ''
  return `${sanitizedUrl.slice(0, queryStart)}${queryTemplate}`
}

function typeForKeyValue(item: KeyValue): string {
  if (item.kind === 'file') return 'file'
  return 'string'
}

function descriptionForKeyValue(item: KeyValue): string {
  const sensitive = item.kind === 'secret' || isSensitiveName(item.key)
  if (!sensitive) return item.description || NO_PARAMETER_DESCRIPTION
  if (!item.description) return '敏感字段，不展示字段值'
  return item.description.includes('不展示字段值')
    ? item.description
    : `${item.description}（敏感字段，不展示字段值）`
}

function row(
  name: string,
  type: string,
  description = '',
  enabled = true,
  status?: string,
): RequestDocRow {
  return {
    name: name || '未命名参数',
    type: type || 'unknown',
    description: description || NO_PARAMETER_DESCRIPTION,
    status: status ?? (enabled ? '已启用' : '未启用'),
  }
}

function rowsFromKeyValues(rows: KeyValue[] | undefined): RequestDocRow[] {
  return (rows ?? [])
    .filter((item) => item.key || item.description)
    .map((item) => row(item.key, typeForKeyValue(item), descriptionForKeyValue(item), item.enabled))
}

function treeRowsFromKeyValues(rows: KeyValue[] | undefined): RequestDocTreeRow[] {
  return (rows ?? [])
    .filter((item) => item.key || item.description)
    .map((item) => ({
      ...row(item.key, typeForKeyValue(item), descriptionForKeyValue(item), item.enabled),
      depth: 0,
      hasChildren: false,
    }))
}

function isObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

function jsonType(value: unknown): string {
  if (value === null) return 'null'
  if (Array.isArray(value)) {
    if (!value.length) return 'array'
    return `array<${jsonType(value[0])}>`
  }
  if (isObject(value)) return 'object'
  return typeof value
}

function jsonDescription(name: string): string {
  return isSensitiveName(name)
    ? '敏感字段，不展示字段值'
    : NO_PARAMETER_DESCRIPTION
}

function jsonTreeRows(content: string): { rows: RequestDocTreeRow[]; valid: boolean } {
  if (!content.trim()) return { rows: [], valid: true }

  try {
    const parsed: unknown = JSON.parse(content)
    const rows: RequestDocTreeRow[] = []

    function visit(name: string, value: unknown, depth: number) {
      const children: Array<[string, unknown]> = isObject(value)
        ? Object.entries(value)
        : Array.isArray(value) && value.length
          ? [['items', value[0]]]
          : []
      rows.push({
        ...row(name, jsonType(value), jsonDescription(name)),
        depth,
        hasChildren: children.length > 0,
      })
      for (const [childName, childValue] of children) visit(childName, childValue, depth + 1)
    }

    if (isObject(parsed)) {
      for (const [name, value] of Object.entries(parsed)) visit(name, value, 0)
    } else {
      visit('body', parsed, 0)
    }
    return { rows, valid: true }
  } catch {
    return { rows: [], valid: false }
  }
}

function tableSection(id: string, title: string, rows: RequestDocRow[]): RequestDocSection {
  return { id, title, kind: 'table', count: rows.length, rows }
}

function treeSection(id: string, title: string, tree: RequestDocTreeRow[], detail?: string): RequestDocSection {
  return {
    id,
    title,
    kind: 'tree',
    count: tree.filter((row) => row.depth === 0).length,
    tree,
    detail,
  }
}

function summarySection(id: string, title: string, value: string, detail?: string): RequestDocSection {
  return { id, title, kind: 'summary', value, detail }
}

function bodySections(body: Body): RequestDocSection[] {
  switch (body.type) {
    case 'none':
      return [summarySection('body', 'Body', '无请求体')]
    case 'json': {
      const result = jsonTreeRows(body.content)
      if (!result.valid) {
        return [summarySection('body', 'Body · JSON', '无法生成参数树', 'JSON 格式无效，请在编辑模式检查请求体。')]
      }
      return [treeSection('body', 'Body 参数 · JSON', result.rows, result.rows.length ? undefined : '暂无 JSON 参数定义。')]
    }
    case 'text':
      return [summarySection('body', `Body · ${body.contentType || 'raw'}`, '原始请求体', 'raw 类型无法从当前内容推导结构化参数树。')]
    case 'formData':
      return [treeSection('body', 'Body 参数 · form-data', treeRowsFromKeyValues(body.rows), '参数说明取自 Body 配置。')]
    case 'urlEncoded':
      return [treeSection('body', 'Body 参数 · x-www-form-urlencoded', treeRowsFromKeyValues(body.rows), '参数说明取自 Body 配置。')]
    case 'binary':
      return [summarySection('body', 'Body · binary', '二进制请求体', '本地文件引用，不展示文件路径或示例内容。')]
    case 'graphQl': {
      const sections = [summarySection('body', 'Body · GraphQL', 'GraphQL 请求体', '查询文本不作为示例展示。')]
      const variables = jsonTreeRows(body.variables)
      if (variables.valid && variables.rows.length) {
        sections.push(treeSection('body-variables', 'Variables 参数', variables.rows))
      } else if (!variables.valid) {
        sections.push(summarySection('body-variables', 'Variables 参数', '无法生成参数树', 'Variables 不是有效 JSON。'))
      }
      return sections
    }
  }
}

function authSection(auth: AuthConfig): RequestDocSection {
  switch (auth.type) {
    case 'inherit':
      return summarySection('auth', '鉴权', '继承集合或文件夹配置', '当前请求未覆盖上级鉴权。')
    case 'none':
      return summarySection('auth', '鉴权', '无鉴权')
    case 'basic':
      return tableSection('auth', '鉴权 · Basic Auth', [
        row('username', 'string', 'Basic Auth 用户名'),
        row('password', 'string', 'Basic Auth 密码，不展示凭据'),
      ])
    case 'bearer':
      return tableSection('auth', '鉴权 · Bearer Token', [
        row('token', 'string', 'Bearer Token，不展示凭据'),
      ])
    case 'apiKey': {
      const location = auth.location === 'header' ? 'Header' : auth.location === 'query' ? 'Query' : 'Cookie'
      return tableSection('auth', '鉴权 · API Key', [
        row(auth.key || 'apiKey', 'string', `API Key 参数，位置：${location}`),
      ])
    }
  }
}

function settingsSection(settings: RequestSettings): RequestDocSection | null {
  const rows: RequestDocRow[] = []
  if (settings.timeoutMs != null) rows.push(row('timeout', 'number', '请求级超时覆盖'))
  if (settings.maxRedirects != null) rows.push(row('maxRedirects', 'number', '请求级重定向次数覆盖'))
  if (settings.followRedirects != null) rows.push(row('followRedirects', 'boolean', '是否跟随重定向'))
  if (settings.verifyTls != null) rows.push(row('verifyTls', 'boolean', '是否校验 TLS 证书'))
  if (settings.httpVersion != null) rows.push(row('httpVersion', 'enum', 'HTTP 协议版本'))
  if (settings.encodeUrl != null) rows.push(row('encodeUrl', 'boolean', '是否编码 URL'))
  if (settings.sendCookies != null) rows.push(row('sendCookies', 'boolean', '是否发送 Cookies'))
  return rows.length ? tableSection('settings', '请求设置', rows) : null
}

/**
 * Build a developer-facing integration view from the in-memory Tab. The
 * returned model intentionally excludes request values and body examples.
 */
export function buildRequestDocumentation(tab: Tab): RequestDocViewModel {
  const pathRows = rowsFromKeyValues(tab.pathParams)
  const queryRows = rowsFromKeyValues(tab.params)
  const headerRows = rowsFromKeyValues(tab.headers)
  const sections: RequestDocSection[] = []

  if (pathRows.length) sections.push(tableSection('path', 'Path 参数', pathRows))
  if (queryRows.length) sections.push(tableSection('query', 'Query 参数', queryRows))
  if (headerRows.length) sections.push(tableSection('headers', 'Headers', headerRows))
  sections.push(...bodySections(tab.body))
  sections.push(authSection(tab.auth))
  const settings = settingsSection(tab.settings)
  if (settings) sections.push(settings)

  const hasConfiguration = Boolean(
    pathRows.length
    || queryRows.length
    || headerRows.length
    || tab.body.type !== 'none'
    || (tab.auth.type !== 'inherit' && tab.auth.type !== 'none')
    || settings,
  )
  const secretQueryNames = (tab.params ?? [])
    .filter((item) => item.kind === 'secret')
    .map((item) => item.key)
  const url = sanitizeUrlTemplate(tab.url, secretQueryNames, tab.pathParams)
  return {
    title: tab.title || '未命名请求',
    method: tab.method.toUpperCase() || 'GET',
    url: url || '未设置 URL',
    copyUrl: url,
    dirty: tab.dirty,
    isEmpty: !hasConfiguration,
    sections: hasConfiguration ? sections : [],
  }
}
