import type { Body, KeyValue } from '@/ipc'

export type ParameterFormat = 'query' | 'headers' | 'form'
export interface ParameterParseError { line: number; message: string }
export interface ParameterParseResult { rows: KeyValue[]; errors: ParameterParseError[] }

/** URL 中的三种 Path 占位符；authority、Query 和 fragment 不参与识别。 */
export function pathParameterNames(url: string): string[] {
  const address = url.trim().split(/[?#]/, 1)[0] ?? ''
  const authority = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.exec(address)?.[0].length
    ?? (address.startsWith('//') ? 2 : 0)
  const start = address.startsWith('/') && !authority ? 0 : address.indexOf('/', authority)
  if (start < 0) return []
  const path = address.slice(start)
  const names = new Set<string>()
  const pattern = /\{\{([A-Za-z_][A-Za-z0-9_-]*)\}\}|\{([A-Za-z_][A-Za-z0-9_-]*)\}|:([A-Za-z_][A-Za-z0-9_-]*)/g
  for (const match of path.matchAll(pattern)) {
    const before = path[match.index - 1]
    const after = path[match.index + match[0].length]
    if (match[3] ? before !== '/' : before === '{' || after === '}') continue
    const name = match[1] ?? match[2] ?? match[3]
    if (name) names.add(name)
  }
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
  const append = (key: string, value: string, enabled: boolean, description?: string, kind?: KeyValue['kind']) => {
    result.rows.push({ enabled, key, value, ...(description === undefined ? {} : { description }), ...(kind === undefined ? {} : { kind }) })
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
  result.rows = preserveParameterMetadata(result.rows, previous)
  return result
}

export function convertFormBody(body: Body, type: 'formData' | 'urlEncoded'): Body {
  const rows = body.type === 'formData' || body.type === 'urlEncoded' ? body.rows : []
  if (type === 'urlEncoded' && rows.some((row) => row.kind === 'file')) {
    throw {
      kind: 'parse',
      message: '文件参数不能转换为 URL 编码表单',
      hints: [{ label: '先移除文件参数，或明确将其改成文本后再切换类型' }],
    }
  }
  return { type, rows }
}


function preserveParameterMetadata(rows: KeyValue[], original: KeyValue[]): KeyValue[] {
  const exact = new Map<string, number[]>()
  const byKey = new Map<string, number[]>()
  const byValue = new Map<string, number[]>()
  const signature = (row: KeyValue) => JSON.stringify([row.key, row.value])
  // 反向入栈，使重复参数按原始顺序取出，避免 shift 的二次复杂度。
  for (let index = original.length - 1; index >= 0; index--) {
    const row = original[index]!
    for (const [map, key] of [[exact, signature(row)], [byKey, row.key], [byValue, row.value]] as const) {
      const bucket = map.get(key) ?? []
      bucket.push(index)
      map.set(key, bucket)
    }
  }
  const used = new Set<number>()
  function take(bucket: number[] | undefined): number | undefined {
    while (bucket?.length) {
      const index = bucket.pop()!
      if (!used.has(index)) { used.add(index); return index }
    }
  }
  const matches = rows.map((row) => take(exact.get(signature(row))))
  rows.forEach((row, index) => { matches[index] ??= take(byKey.get(row.key)) })
  rows.forEach((row, index) => { matches[index] ??= take(byValue.get(row.value)) })
  const unmatchedSecret = original.some((row, index) => row.kind === 'secret' && !used.has(index))
  return rows.map((row, index) => {
    let source = matches[index]
    if (source === undefined && original[index] && !used.has(index)) {
      source = index
      used.add(index)
    }
    // 同时改名、改值、移动时文本没有行身份；宁可多保留敏感标记，也不能静默取消脱敏。
    const kind = row.kind ?? (matches[index] === undefined && unmatchedSecret ? 'secret' : original[source ?? -1]?.kind ?? 'text')
    return { description: '', ...(source === undefined ? {} : original[source]), ...row, kind }
  })
}
