export interface PreviewVariable {
  key: string
  value: string
}

export interface VariablePreview {
  value: string
  error?: string
}

const MAX_DEPTH = 16

export function resolveVariablePreview(
  name: string,
  variables: PreviewVariable[],
): VariablePreview {
  const values = new Map(variables.map((variable) => [variable.key.trim(), variable.value]))
  const stack: string[] = []

  function resolveText(text: string, depth: number): VariablePreview {
    if (depth > MAX_DEPTH) return { value: '', error: '嵌套层级过深' }
    let error: string | undefined
    const value = text.replace(/\{\{([^{}]+)\}\}/g, (_match, rawName: string) => {
      const key = rawName.trim()
      if (!key) {
        error = '变量名为空'
        return ''
      }
      if (stack.includes(key)) {
        error = `循环引用：${[...stack, key].join(' -> ')}`
        return ''
      }
      const raw = values.get(key)
      if (raw === undefined) {
        error = `未定义变量：${key}`
        return ''
      }
      stack.push(key)
      const resolved = resolveText(raw, depth + 1)
      stack.pop()
      if (resolved.error) error = resolved.error
      return resolved.value
    })
    return error ? { value, error } : { value }
  }

  const raw = values.get(name)
  if (raw === undefined) return { value: '', error: `未定义变量：${name}` }
  stack.push(name)
  const result = resolveText(raw, 0)
  stack.pop()
  return result
}
