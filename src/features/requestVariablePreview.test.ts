import { describe, expect, it } from 'vitest'
import type { Environment, EnvironmentVariable, WorkspaceNode } from '@/ipc'
import { requestVariablePreview } from './requestVariablePreview'

const variable = (id: string, key: string, value: string): EnvironmentVariable => ({
  id, key, value, enabled: true, secret: false,
})

const node = (id: string, parentId: string | null, kind: WorkspaceNode['kind'], variables: EnvironmentVariable[] = []): WorkspaceNode => ({
  id, parentId, depth: 0, name: id, isFolder: kind !== 'request', expanded: false,
  kind, debugCaseCount: 0, variables,
})

describe('request variable preview', () => {
  it('keeps global and collection suggestions when no environment is selected', () => {
    const nodes = [
      node('collection', null, 'collection', [variable('collection-x', 'x', 'collection')]),
      node('request', 'collection', 'request'),
    ]
    const suggestions = requestVariablePreview([variable('global-y', 'y', 'global')], nodes, null, 'request')
    expect(suggestions.map(({ key, value }) => [key, value])).toEqual([
      ['y', 'global'],
      ['x', 'collection'],
    ])
  })

  it('uses the same inner-scope, environment, and built-in host precedence as sends', () => {
    const nodes = [
      node('collection', null, 'collection', [variable('collection-x', 'x', 'collection')]),
      node('folder', 'collection', 'folder', [variable('folder-x', 'x', 'folder')]),
      node('request', 'folder', 'request'),
    ]
    const environment: Environment = {
      id: 'env', name: 'Staging', host: 'https://staging.example.test/',
      variables: [variable('environment-x', 'x', 'environment')],
    }
    const suggestions = requestVariablePreview([variable('global-x', 'x', 'global')], nodes, environment, 'request')
    expect(suggestions.find(({ key }) => key === 'x')?.value).toBe('environment')
    expect(suggestions.find(({ key }) => key === 'host')?.value).toBe('https://staging.example.test')
    expect(suggestions.filter(({ key }) => key.toLowerCase() === 'host')).toHaveLength(1)
  })
})
