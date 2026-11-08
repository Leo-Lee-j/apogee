import { describe, expect, it } from 'vitest'
import type { KeyValue } from '@/ipc'
import { createParameterRow, parameterRowsEqual, parseParameterRows, pathParameterNames, serializeParameterRows } from './requestParameters'

const row = (key: string, value: string, extra: Partial<KeyValue> = {}): KeyValue => ({
  enabled: true, key, value, description: '', kind: 'text', ...extra,
})

describe('parameter row submission', () => {
  it('commits key, value, description and file kind as one independent row', () => {
    const draft = { key: 'upload', value: 'C:\\temp\\a.txt', description: 'Attachment', kind: 'file' as const }
    const created = createParameterRow(draft)
    expect(created).toEqual({ enabled: true, ...draft })
    draft.key = ''
    expect(created?.key).toBe('upload')
  })
  it('does not create a row for an empty draft or a kind-only selection', () => {
    expect(createParameterRow({})).toBeNull()
    expect(createParameterRow({ kind: 'file' })).toBeNull()
  })
  it('supports value-only and description-only drafts without losing fields', () => {
    expect(createParameterRow({ value: '0' })).toEqual(row('', '0'))
    expect(createParameterRow({ description: 'note' })).toEqual(row('', '', { description: 'note' }))
  })
  it('avoids dirty marks for semantically identical parameter arrays', () => {
    expect(parameterRowsEqual([{ enabled: true, key: 'k', value: 'v' }], [row('k', 'v')])).toBe(true)
    expect(parameterRowsEqual([row('k', 'v')], [row('k', 'v', { enabled: false })])).toBe(false)
    expect(parameterRowsEqual([row('k', 'v')], [row('k', 'v', { kind: 'secret' })])).toBe(false)
    expect(parameterRowsEqual([row('k', 'v')], [row('k', 'v', { description: 'updated' })])).toBe(false)
    expect(parameterRowsEqual([row('k', 'v'), row('x', 'y')], [row('x', 'y'), row('k', 'v')])).toBe(false)
  })
})

describe('bulk parameter editing', () => {
  it('round-trips descriptions, disabled rows, secret/file metadata and duplicate names', () => {
    const rows = [row('id', '1', { description: 'first', kind: 'secret' }), row('id', '2', { enabled: false, kind: 'file' })]
    expect(parseParameterRows(serializeParameterRows(rows)).rows).toEqual(rows)
  })
  it('round-trips whitespace, separators, newlines, literal escapes and // keys', () => {
    const rows = [row('// key:with=separators', ' \tline\nnext\r\nC:\\temp\\a.txt ', { description: ' \nquoted "text"\t ' }), row('', '')]
    const result = parseParameterRows(serializeParameterRows(rows))
    expect(result.errors).toEqual([])
    expect(result.rows).toEqual(rows)
  })
  it('imports key: value and disabled lines while keeping duplicate-row metadata', () => {
    const previous = [row('id', 'old', { kind: 'secret', description: 'first' }), row('id', 'old2', { description: 'second' })]
    const result = parseParameterRows('id: new\n// id: other\n\n', previous)
    expect(result.rows).toEqual([row('id', 'new', { kind: 'secret', description: 'first' }), row('id', 'other', { enabled: false, description: 'second' })])
    expect(previous[0].value).toBe('old')
  })
  it('parses ordinary TSV and explicitly clears an existing description', () => {
    expect(parseParameterRows('k\tv\t', [row('k', 'old', { description: 'old', kind: 'secret' })]).rows)
      .toEqual([row('k', 'v', { kind: 'secret' })])
    expect(parseParameterRows('upload\tC:\\temp\\a.txt\tAttachment\tfile').rows)
      .toEqual([row('upload', 'C:\\temp\\a.txt', { description: 'Attachment', kind: 'file' })])
  })
  it('supports empty values and a blank optional type column', () => {
    expect(parseParameterRows('k\t').rows).toEqual([row('k', '')])
    expect(parseParameterRows('k\tv\t\t').rows).toEqual([row('k', 'v')])
  })
  it('decodes Query Strings without dropping duplicates, empty values or placeholders', () => {
    const result = parseParameterRows('?tag=a&tag=b&q=hello+world&symbol=%2B&empty=&flag&token={{token}}')
    expect(result.rows).toEqual([
      row('tag', 'a'), row('tag', 'b'), row('q', 'hello world'), row('symbol', '+'),
      row('empty', ''), row('flag', ''), row('token', '{{token}}'),
    ])
  })
  it('imports full URLs without importing userinfo, paths or fragments', () => {
    const result = parseParameterRows('https://user:password@api.test/users?q=%E4%B8%AD%E6%96%87&page=1#ignored')
    expect(result.rows).toEqual([row('q', '中文'), row('page', '1')])
    expect(parseParameterRows('https://api.test/users').errors).toHaveLength(1)
    expect(parseParameterRows('https://api.test/users#section?not_query=1').errors).toHaveLength(1)
  })
  it('keeps URLs, plus signs, percent escapes and equals signs literal in headers', () => {
    const result = parseParameterRows('Referer: https://api.test/?q=a+b%20c\nX-Token=prefix:abc==', [], 'headers')
    expect(result.rows).toEqual([row('Referer', 'https://api.test/?q=a+b%20c'), row('X-Token', 'prefix:abc==')])
  })
  it('handles CRLF, disabled query strings and standalone empty-valued keys', () => {
    expect(parseParameterRows('// a=1&b=2\r\nflag\r\n').rows)
      .toEqual([row('a', '1', { enabled: false }), row('b', '2', { enabled: false }), row('flag', '')])
  })
  it('reports line numbers without including secret input in errors', () => {
    const result = parseParameterRows('valid: yes\nkey\tsecret-value\tdesc\tunsupported\n: private-password')
    expect(result.errors.map(error => error.line)).toEqual([2, 3])
    expect(JSON.stringify(result.errors)).not.toContain('secret-value')
    expect(JSON.stringify(result.errors)).not.toContain('private-password')
  })
  it('rejects extra columns and malformed quoted fields rather than silently truncating', () => {
    expect(parseParameterRows('a\tb\tc\ttext\textra').errors).toHaveLength(1)
    expect(parseParameterRows('"unterminated\tv').errors).toHaveLength(1)
  })
})

describe('Path parameter discovery', () => {
  it('supports all existing placeholder syntaxes and deduplicates in path order', () => {
    expect(pathParameterNames('https://api.test/:id/{name}/{{id}}/:identifier')).toEqual(['id', 'name', 'identifier'])
  })
  it('ignores placeholders and colon names in the authority, query and fragment', () => {
    expect(pathParameterNames('https://user:password@{{host}}:port/:id?q={{query}}#/:fragment')).toEqual(['id'])
    expect(pathParameterNames('{{host}}')).toEqual([])
    expect(pathParameterNames('https://api.test?next=/:query')).toEqual([])
  })
  it('supports relative paths and URLs without a scheme', () => {
    expect(pathParameterNames('/users/{id}?q=:query')).toEqual(['id'])
    expect(pathParameterNames('api.test/users/:id')).toEqual(['id'])
    expect(pathParameterNames('{{host}}/users/{id}')).toEqual(['id'])
  })
})
