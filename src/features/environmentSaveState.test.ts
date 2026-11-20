import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import { useEnvironmentSaveState } from './environmentSaveState'

function fixture() {
  const workspace = reactive({
    workspaceId: 'workspace', sessionId: 'session', activeEnvironmentDirty: false, globalVariablesDirty: false,
    saveEnvironment: vi.fn(async () => { workspace.activeEnvironmentDirty = false; return true }),
    saveGlobalVariables: vi.fn(async () => { workspace.globalVariablesDirty = false; return true }),
  })
  return { workspace, state: useEnvironmentSaveState(workspace, () => true) }
}

describe('environment dialog save state', () => {
  it('shows and saves a global-only edit with no active environment', async () => {
    const { workspace, state } = fixture()
    workspace.globalVariablesDirty = true
    expect(state.dirty.value).toBe(true)
    expect(state.status.value).toBe('有未保存修改')
    await expect(state.save()).resolves.toBe(true)
    expect(workspace.saveEnvironment).not.toHaveBeenCalled()
    expect(state.status.value).toBe('已保存')
  })

  it('shows saving and flushes environment edits made during a global save', async () => {
    const { workspace, state } = fixture()
    let complete!: (value: boolean) => void
    workspace.globalVariablesDirty = true
    workspace.saveGlobalVariables.mockImplementationOnce(() => new Promise(resolve => { complete = resolve }))
    const saving = state.save()
    expect(state.save()).toBe(saving)
    expect(state.status.value).toBe('保存中…')
    workspace.activeEnvironmentDirty = true
    workspace.globalVariablesDirty = false
    complete(true)
    await expect(saving).resolves.toBe(true)
    expect(workspace.saveEnvironment).toHaveBeenCalledOnce()
    expect(state.status.value).toBe('已保存')
  })

  it('keeps unsaved feedback after a failed save and permits retry', async () => {
    const { workspace, state } = fixture()
    workspace.globalVariablesDirty = true
    workspace.saveGlobalVariables.mockResolvedValueOnce(false)
    await expect(state.save()).resolves.toBe(false)
    expect(state.saving.value).toBe(false)
    expect(state.status.value).toBe('保存失败，有未保存修改')
    await expect(state.save()).resolves.toBe(true)
    expect(state.status.value).toBe('已保存')
  })

  it('does not complete a save for a replacement workspace session', async () => {
    const { workspace, state } = fixture()
    workspace.globalVariablesDirty = true
    workspace.saveGlobalVariables.mockImplementationOnce(async () => {
      workspace.sessionId = 'new-session'
      workspace.globalVariablesDirty = false
      return true
    })
    await expect(state.save()).resolves.toBe(false)
    expect(state.saving.value).toBe(false)
  })
})
