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
})
