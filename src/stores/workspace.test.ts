import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { Environment, EnvironmentVariable, WorkspaceSnapshot } from '@/ipc'

const { workspaceSaveEnvironment, workspaceSaveGlobalVariables, workspaceSaveState, workspaceLoad } = vi.hoisted(() => ({
  workspaceSaveEnvironment: vi.fn(),
  workspaceSaveGlobalVariables: vi.fn(),
  workspaceSaveState: vi.fn(),
  workspaceLoad: vi.fn(),
}))

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { workspaceSaveEnvironment, workspaceSaveGlobalVariables, workspaceSaveState, workspaceLoad },
  normalizeError: (error: unknown) => ({
    kind: 'io',
    message: error instanceof Error ? error.message : String(error),
    hints: [{ label: '检查目录权限后重试' }],
  }),
}))

import { useWorkspaceStore } from './workspace'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}

function prepareWorkspace() {
  const workspace = useWorkspaceStore()
  workspace.workspaceId = 'workspace-a'
  workspace.sessionId = 'session-a'
  workspace.ready = true
  workspace.environments = [{ id: 'env', name: '开发', host: 'https://old.test', variables: [] }]
  workspace.activeEnvironmentId = 'env'
  workspace.updateEnvironment('env', { host: 'https://first.test' })
  return workspace
}

describe('workspace persistence', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.resetAllMocks()
    vi.spyOn(console, 'error').mockImplementation(() => {})
    workspaceSaveEnvironment.mockImplementation(async (environment: Environment) => environment)
    workspaceSaveGlobalVariables.mockImplementation(async (variables: EnvironmentVariable[]) => variables)
  })

  afterEach(() => vi.restoreAllMocks())

  it('serializes global saves and flushes edits made while saving', async () => {
    const first = deferred<EnvironmentVariable[]>()
    const second = deferred<EnvironmentVariable[]>()
    workspaceSaveGlobalVariables.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const workspace = prepareWorkspace()
    workspace.globalVariables = [{ id: 'global', key: 'api', value: 'before', enabled: true, secret: false }]
    workspace.updateGlobalVariable(0, { value: 'first' })
    const saving = workspace.saveGlobalVariables()
    const sending = workspace.saveDirtyEnvironments()
    expect(workspaceSaveGlobalVariables).toHaveBeenCalledTimes(1)
    workspace.updateGlobalVariable(0, { value: 'latest' })
    first.resolve(workspaceSaveGlobalVariables.mock.calls[0]![0])
    await vi.waitFor(() => expect(workspaceSaveGlobalVariables).toHaveBeenCalledTimes(2))
    expect(workspace.globalVariables[0]!.value).toBe('latest')
    second.resolve(workspaceSaveGlobalVariables.mock.calls[1]![0])
    await expect(saving).resolves.toBe(true)
    await expect(sending).resolves.toBe(true)
    expect(workspace.globalVariablesDirty).toBe(false)
  })

  it.each(['resolve', 'reject'] as const)('ignores a global save from a closed session that later %ss', async (outcome) => {
    const pending = deferred<EnvironmentVariable[]>()
    workspaceSaveGlobalVariables.mockReturnValueOnce(pending.promise)
    const workspace = prepareWorkspace()
    workspace.addGlobalVariable()
    workspace.updateGlobalVariable(0, { key: 'old', value: 'old-value' })
    const saving = workspace.saveDirtyEnvironments()
    workspace.reset()
    workspace.workspaceId = 'workspace-b'
    workspace.sessionId = 'session-b'
    workspace.addGlobalVariable()
    workspace.updateGlobalVariable(0, { key: 'new', value: 'new-value' })
    if (outcome === 'resolve') pending.resolve(workspaceSaveGlobalVariables.mock.calls[0]![0])
    else pending.reject(new Error('old global save failed'))
    await expect(saving).resolves.toBe(false)
    expect(workspace.globalVariables[0]!.value).toBe('new-value')
    expect(workspace.globalVariablesDirty).toBe(true)
    expect(workspaceSaveEnvironment).not.toHaveBeenCalled()
    expect(workspace.lastError).toBeNull()
  })

  it('preserves edits made during a save and flushes them before reporting success', async () => {
    const first = deferred<Environment>()
    const second = deferred<Environment>()
    workspaceSaveEnvironment.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const workspace = prepareWorkspace()
    const saving = workspace.saveDirtyEnvironments()
    const firstSnapshot = workspaceSaveEnvironment.mock.calls[0]![0] as Environment

    workspace.updateEnvironment('env', { host: 'https://latest.test' })
    first.resolve(firstSnapshot)
    await vi.waitFor(() => expect(workspaceSaveEnvironment).toHaveBeenCalledTimes(2))

    expect(workspace.activeEnvironment?.host).toBe('https://latest.test')
    expect(workspace.activeEnvironmentDirty).toBe(true)
    second.resolve(workspaceSaveEnvironment.mock.calls[1]![0] as Environment)
    await expect(saving).resolves.toBe(true)
    expect(workspace.activeEnvironmentDirty).toBe(false)
  })

  it('shares an in-flight environment save between the editor and request sending', async () => {
    const pending = deferred<Environment>()
    workspaceSaveEnvironment.mockReturnValueOnce(pending.promise)
    const workspace = prepareWorkspace()

    const editorSave = workspace.saveEnvironment()
    const sendingSave = workspace.saveDirtyEnvironments()
    expect(workspaceSaveEnvironment).toHaveBeenCalledTimes(1)
    pending.resolve(workspaceSaveEnvironment.mock.calls[0]![0] as Environment)

    await expect(editorSave).resolves.toBe(true)
    await expect(sendingSave).resolves.toBe(true)
  })

  it('keeps newer edits dirty after a failed save so a retry can persist them', async () => {
    const pending = deferred<Environment>()
    workspaceSaveEnvironment.mockReturnValueOnce(pending.promise)
    const workspace = prepareWorkspace()
    const saving = workspace.saveEnvironment()
    workspace.updateEnvironment('env', { host: 'https://latest.test' })
    pending.reject(new Error('disk full'))

    await expect(saving).resolves.toBe(false)
    expect(workspace.activeEnvironment?.host).toBe('https://latest.test')
    expect(workspace.activeEnvironmentDirty).toBe(true)
    expect(workspace.lastError?.message).toBe('disk full')
    await expect(workspace.saveEnvironment()).resolves.toBe(true)
    expect(workspaceSaveEnvironment.mock.calls[1]![0].host).toBe('https://latest.test')
  })

  it.each(['resolve', 'reject'] as const)('ignores an old session save that later %ss', async (outcome) => {
    const pending = deferred<Environment>()
    workspaceSaveEnvironment.mockReturnValueOnce(pending.promise)
    const workspace = prepareWorkspace()
    const saving = workspace.saveEnvironment()
    const oldEnvironment = workspaceSaveEnvironment.mock.calls[0]![0] as Environment
    workspace.reset()
    workspace.workspaceId = 'workspace-a'
    workspace.sessionId = 'session-reopened'
    workspace.environments = [{ ...oldEnvironment, host: 'https://reopened.test' }]
    workspace.activeEnvironmentId = 'env'
    workspace.updateEnvironment('env', { name: '重新打开后的编辑' })

    if (outcome === 'resolve') pending.resolve(oldEnvironment)
    else pending.reject(new Error('old session failed'))

    await expect(saving).resolves.toBe(false)
    expect(workspace.activeEnvironment?.host).toBe('https://reopened.test')
    expect(workspace.activeEnvironmentDirty).toBe(true)
    expect(workspace.lastError).toBeNull()
  })

  it('does not install a stale workspace load or clear the newer loading state', async () => {
    const oldLoad = deferred<WorkspaceSnapshot>()
    const newLoad = deferred<WorkspaceSnapshot>()
    workspaceLoad.mockReturnValueOnce(oldLoad.promise).mockReturnValueOnce(newLoad.promise)
    const workspace = prepareWorkspace()
    const loadingOld = workspace.load()
    workspace.reset()
    workspace.workspaceId = 'workspace-b'
    workspace.sessionId = 'session-b'
    const loadingNew = workspace.load()

    oldLoad.resolve({ id: 'workspace-a', name: 'A', nodes: [], environments: [] })
    await loadingOld
    expect(workspace.workspaceId).toBe('workspace-b')
    expect(workspace.loading).toBe(true)

    newLoad.resolve({ id: 'workspace-b', name: 'B', nodes: [], environments: [] })
    await loadingNew
    expect(workspace.workspaceName).toBe('B')
    expect(workspace.loading).toBe(false)
  })

  it('discards queued environment selections from a previous session', async () => {
    const pending = deferred<void>()
    workspaceSaveState.mockReturnValueOnce(pending.promise)
    const workspace = prepareWorkspace()
    const first = workspace.persistWorkspaceState()
    await vi.waitFor(() => expect(workspaceSaveState).toHaveBeenCalledTimes(1))
    workspace.activeEnvironmentId = null
    const queued = workspace.persistWorkspaceState()
    workspace.reset()
    workspace.workspaceId = 'workspace-b'
    workspace.sessionId = 'session-b'

    pending.resolve()
    await expect(first).resolves.toBe(false)
    await expect(queued).resolves.toBe(false)
    expect(workspaceSaveState).toHaveBeenCalledTimes(1)
    await expect(workspace.flushWorkspaceState()).resolves.toBe(true)
  })
})
