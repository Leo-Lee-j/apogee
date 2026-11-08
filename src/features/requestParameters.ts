import type { KeyValue } from '@/ipc'

export type ParameterFormat = 'query' | 'headers' | 'form'
export interface ParameterParseError { line: number; message: string }
export interface ParameterParseResult { rows: KeyValue[]; errors: ParameterParseError[] }

/** 只扫描 URL 的路径，不把 Host、userinfo、Query 或 fragment 识别成 Path 参数。 */
export function pathParameterNames(url: string): string[] {
  const head = url.split(/[?#]/, 1)[0] ?? ''
  const scheme = head.indexOf('://')
  const start = head.startsWith('/') ? 0 : head.indexOf('/', scheme < 0 ? 0 : scheme + 3)
  if (start < 0) return []
  const names = new Set<string>()
  const pattern = /\{\{([A-Za-z_][A-Za-z0-9_-]*)\}\}|:([A-Za-z_][A-Za-z0-9_-]*)|(?<!\{)\{([A-Za-z_][A-Za-z0-9_-]*)\}(?!\})/g
  for (const match of head.slice(start).matchAll(pattern)) names.add(match[1] ?? match[2] ?? match[3])
  return [...names]
}

/** 新行一次提交全部字段，不能依赖父组件下一次渲染才能合并字段。 */
export function createParameterRow(draft: Partial<KeyValue>): KeyValue | null {
  if (!draft.key && !draft.value && !draft.description) return null
  return {
    enabled: draft.enabled ?? true,
    key: draft.key ?? '',
    value: draft.value ?? '',
    description: draft.description ?? '',
    kind: draft.kind ?? 'text',
  }
}

export function parameterRowsEqual(left: KeyValue[], right: KeyValue[]): boolean {
  return left.length === right.length && left.every((row, index) => {
    const other = right[index]
    return other != null && row.enabled === other.enabled && row.key === other.key
      && row.value === other.value && (row.description ?? '') === (other.description ?? '')
      && (row.kind ?? 'text') === (other.kind ?? 'text')
  })
}

/** 带引号的 TSV 字段能无损表示 Tab、换行、反斜杠及 // 开头的参数名。 */
export function serializeParameterRows(rows: KeyValue[]): string {
  return rows.map((row) => {
    const simple = /^[A-Za-z0-9_.-]+$/.test(row.key) && !row.description
      && (row.kind ?? 'text') === 'text' && row.value === row.value.trim() && !/[\t\r\n]/.test(row.value)
    if (simple) {
      const pair = `${row.key}: ${row.value}`
      return row.enabled ? pair : `// ${pair}`
    }
    const fields = [row.key, row.value, row.description ?? '', row.kind ?? 'text']
      .map((field) => JSON.stringify(field)).join('\t')
    return row.enabled ? fields : `// ${fields}`
  }).join('\n')
}

function columnValue(value: string): string {
  const trimmed = value.trim()
  if (!trimmed.startsWith('"')) return value
  const parsed: unknown = JSON.parse(trimmed)
  if (typeof parsed !== 'string') throw new Error('invalid column')
  return parsed
}

/** 支持 quoted TSV、普通 TSV、key: value、Query String 和完整 URL 的查询部分。 */
export function parseParameterRows(
  text: string,
  previous: KeyValue[] = [],
  format: ParameterFormat = 'query',
): ParameterParseResult {
  const result: ParameterParseResult = { rows: [], errors: [] }
  const occurrences = new Map<string, number>()
  const previousByKey = new Map<string, KeyValue[]>()
  for (const row of previous) {
    const values = previousByKey.get(row.key) ?? []
    values.push(row)
    previousByKey.set(row.key, values)
  }
  const append = (key: string, value: string, enabled: boolean, description?: string, kind?: KeyValue['kind']) => {
    const occurrence = occurrences.get(key) ?? 0
    occurrences.set(key, occurrence + 1)
    const original = previousByKey.get(key)?.[occurrence]
    result.rows.push({
      ...original,
      enabled, key, value,
      description: description ?? original?.description ?? '',
      kind: kind ?? original?.kind ?? 'text',
    })
  }
  text.split(/\r?\n/).forEach((line, index) => {
    let raw = line.trim()
    if (!raw) return
    let enabled = true
    if (raw.startsWith('//')) {
      enabled = false
      raw = raw.slice(2).trimStart()
      if (!raw) return
    }
    try {
      if (line.includes('\t')) {
        // 不 trim 掉末尾空列：第三列空说明是显式清空，而非保留旧说明。
        const source = enabled ? line.trimStart() : line.trimStart().slice(2).trimStart()
        const columns = source.replace(/\r$/, '').split('\t').map(columnValue)
        if (columns.length > 4) throw new Error('too many columns')
        const [key, value = '', description, rawKind] = columns
        const kind = rawKind || undefined
        if (kind != null && !['text', 'file', 'secret'].includes(kind)) throw new Error('invalid kind')
        append(key, value, enabled, description, kind as KeyValue['kind'])
        return
      }
      const isUrl = /^[a-z][a-z\d+.-]*:\/\//i.test(raw)
      const colon = raw.indexOf(':')
      const equals = raw.indexOf('=')
      if (format !== 'headers' && (isUrl || raw.startsWith('?') || (equals >= 0 && (colon < 0 || equals < colon)))) {
        const urlHead = raw.split('#', 1)[0]
        if (isUrl && !urlHead.includes('?')) throw new Error('URL has no query')
        const query = isUrl ? urlHead.slice(urlHead.indexOf('?') + 1) : raw.replace(/^\?/, '')
        const fragment = query.indexOf('#')
        const parameters = new URLSearchParams(fragment < 0 ? query : query.slice(0, fragment))
        for (const [key, value] of parameters) append(key, value, enabled)
        return
      }
      const separator = colon < 0 ? equals : equals < 0 ? colon : Math.min(colon, equals)
      const key = separator < 0 ? raw : raw.slice(0, separator).trim()
      if (!key) throw new Error('empty key')
      const value = separator < 0 ? '' : raw.slice(separator + 1).trimStart()
      append(key, value, enabled)
    } catch {
      // 错误信息只说明格式，不把可能含有凭据的原始行拼进消息。
      result.errors.push({ line: index + 1, message: '格式无效，请使用 key: value、Query String 或最多四列的 Tab 分隔文本。' })
    }
  })
  return result
}
