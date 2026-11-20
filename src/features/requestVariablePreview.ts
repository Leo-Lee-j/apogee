import type { Environment, EnvironmentVariable, WorkspaceNode } from '@/ipc'

/** Builds the same global → collection → folder → environment view used by Rust sends. */
export function requestVariablePreview(
  global: EnvironmentVariable[],
  nodes: WorkspaceNode[],
  environment: Environment | null,
  requestNodeId?: string,
  parentNodeId?: string,
): EnvironmentVariable[] {
  const byId = new Map(nodes.map((node) => [node.id, node]))
  let cursor = byId.get(requestNodeId ?? '') ?? byId.get(parentNodeId ?? '')
  const ancestors: WorkspaceNode[] = []
  while (cursor) {
    if (cursor.isFolder) ancestors.push(cursor)
    cursor = cursor.parentId ? byId.get(cursor.parentId) : undefined
  }
  ancestors.reverse()
  const values = new Map<string, EnvironmentVariable>()
  for (const variable of global) if (variable.enabled) values.set(variable.key.trim(), variable)
  for (const scope of ancestors) {
    for (const variable of scope.variables ?? []) if (variable.enabled) values.set(variable.key.trim(), variable)
  }
  if (environment) {
    for (const variable of environment.variables) if (variable.enabled) values.set(variable.key.trim(), variable)
    if (environment.host.trim()) {
      values.set('host', { id: 'builtin-host', key: 'host', value: environment.host.trim().replace(/\/+$/, ''), enabled: true, secret: false })
    }
  }
  return [...values.values()].filter((variable) => variable.key && variable.key.toLocaleLowerCase() !== 'host' || variable.id === 'builtin-host')
}
