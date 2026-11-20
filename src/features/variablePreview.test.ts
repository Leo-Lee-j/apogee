import { describe, expect, it } from 'vitest'
import { resolveVariablePreview } from './variablePreview'

describe('variable preview', () => {
  const variables = [
    { key: 'host', value: 'https://api.example.com' },
    { key: 'version', value: 'v1' },
    { key: 'baseUrl', value: '{{host}}/{{version}}' },
  ]

  it('resolves nested values', () => {
    expect(resolveVariablePreview('baseUrl', variables)).toEqual({ value: 'https://api.example.com/v1' })
  })

  it('reports missing and cyclic values', () => {
    expect(resolveVariablePreview('missing', variables).error).toContain('未定义变量')
    expect(resolveVariablePreview('a', [{ key: 'a', value: '{{b}}' }, { key: 'b', value: '{{a}}' }]).error).toContain('循环引用')
  })

  it('masks direct and repeated indirect secret references', () => {
    const secret = [
      { key: 'private', value: 'synthetic-preview', secret: true },
      { key: 'indirect', value: '{{private}}/{{private}}' },
    ]
    expect(resolveVariablePreview('private', secret)).toEqual({ value: '••••••••' })
    expect(resolveVariablePreview('indirect', secret)).toEqual({ value: '••••••••' })
  })

  it('bounds growing recursive expansions', () => {
    const variables = [
      { key: 'leaf', value: 'x'.repeat(4096) },
      { key: 'first', value: '{{leaf}}'.repeat(32) },
      { key: 'second', value: '{{first}}'.repeat(32) },
    ]
    expect(resolveVariablePreview('second', variables).error).toContain('大小超过限制')
  })

  it('bounds exponential empty expansions', () => {
    const variables = [{ key: 'leaf', value: '' }]
    for (let i = 0; i < 7; i++) variables.push({ key: 'v' + i, value: ('{{' + (i ? 'v' + (i - 1) : 'leaf') + '}}').repeat(16) })
    expect(resolveVariablePreview('v6', variables).error).toContain('次数超过限制')
  })
})
