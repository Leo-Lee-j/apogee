export interface PreviewVariable {
  key: string
  value: string
  secret?: boolean
}

export interface VariablePreview {
  value: string
  error?: string
}

const MAX_DEPTH = 16
const MAX_OUTPUT = 1024 * 1024
const MAX_REFERENCES = 10000

export function resolveVariablePreview(
  name: string,
  variables: PreviewVariable[],
): VariablePreview {
  const values = new Map(variables.map((variable) => [variable.key.trim(), variable]))
  const stack: string[] = []
  let sensitive = false
  let references = 0
  let expansionSize = 0

  function resolveText(text: string, depth: number): VariablePreview {
    if (depth > MAX_DEPTH) return { value: '', error: '嵌套层级过深' }
    if (text.length > MAX_OUTPUT) return { value: '', error: '变量展开大小超过限制' }
    let error: string | undefined
    let outputSize = text.length
    const value = text.replace(/\{\{([^{}]+)\}\}/g, (_match, rawName: string) => {
      if (error) return ''
      if (++references > MAX_REFERENCES) {
        error = '变量展开次数超过限制'
        return ''
      }
      const key = rawName.trim()
      if (!key) {
        error = '变量名为空'
        return ''
      }
      if (stack.includes(key)) {
        error = `循环引用：${[...stack, key].join(' -> ')}`
        return ''
      }
      const variable = values.get(key)
      if (variable === undefined) {
        error = `未定义变量：${key}`
        return ''
      }
      stack.push(key)
      if (variable.secret) {
        sensitive = true
        stack.pop()
        return '••••••••'
      }
      const resolved = resolveText(variable.value, depth + 1)
      stack.pop()
      if (resolved.error) error = resolved.error
      expansionSize += resolved.value.length
      if (expansionSize > 8 * MAX_OUTPUT) {
        error = '变量展开大小超过限制'
        return ''
      }
      outputSize += resolved.value.length - _match.length
      if (outputSize > MAX_OUTPUT) {
        error = '变量展开大小超过限制'
        return ''
      }
      return resolved.value
    })
    return error ? { value: '', error } : { value }
  }

  const variable = values.get(name)
  if (variable === undefined) return { value: '', error: `未定义变量：${name}` }
  if (variable.secret) return { value: '••••••••' }
  stack.push(name)
  const result = resolveText(variable.value, 0)
  stack.pop()
  if (sensitive) return { value: '••••••••' }
  return result
}
