import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const workspaceClose = vi.fn()
const workspaceOpen = vi.fn()
const workspaceList = vi.fn()

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: new Proxy({}, {
    get(_target, key: string) {
      if (key === 'workspaceClose') return workspaceClose
      if (key === 'workspaceOpen') return workspaceOpen
      if (key === 'workspaceList') return workspaceList
      return vi.fn()
    },
  }),
  normalizeError: (error: unknown) => ({
    kind: 'unknown',
    message: error instanceof Error ? error.message : String(error),
  }),
}))

import { useAppStore } from './app'

describe('app workspace lifecycle', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    workspaceClose.mockReset()
    workspaceClose.mockResolvedValue(undefined)
    workspaceOpen.mockReset()
    workspaceList.mockReset()
    workspaceList.mockResolvedValue([])
  })

  it('flushes the active session before closing and returns to launcher', async () => {
    const app = useAppStore()
    app.shell = 'workspace'

    await expect(app.closeWorkspace()).resolves.toBe(true)

    expect(workspaceClose).toHaveBeenCalledTimes(1)
    expect(app.shell).toBe('launcher')
    expect(app.busyWorkspaceId).toBeNull()
  })

  it('keeps the workspace open when Rust rejects close', async () => {
    workspaceClose.mockRejectedValueOnce(new Error('close failed'))
    const app = useAppStore()
    app.shell = 'workspace'

    await expect(app.closeWorkspace()).resolves.toBe(false)

    expect(app.shell).toBe('workspace')
    expect(app.lastError?.message).toBe('close failed')
  })

  it('installs the next workspace only after the old session closes', async () => {
    const app = useAppStore()
    const workspace = (await import('./workspace')).useWorkspaceStore()
    app.shell = 'workspace'
    workspace.workspaceId = 'workspace-a'
    workspaceOpen.mockResolvedValueOnce({
      descriptor: { id: 'workspace-b', name: 'B', path: 'C:/b', lastOpenedAt: 2, status: 'available' },
      workspaceId: 'workspace-b',
      sessionId: 'session-b',
      snapshot: { id: 'workspace-b', name: 'B', nodes: [], environments: [] },
      state: { activeEnvironmentId: null, activeEnvironmentSet: false },
    })

    await expect(app.switchWorkspace('workspace-b')).resolves.toBe(true)

    expect(workspaceClose).toHaveBeenCalledTimes(1)
    expect(workspaceOpen).toHaveBeenCalledWith({ workspaceId: 'workspace-b' })
    expect(app.shell).toBe('workspace')
    expect(workspace.workspaceId).toBe('workspace-b')
    expect(workspace.sessionId).toBe('session-b')
  })

  it('returns to launcher when the next workspace cannot open', async () => {
    const app = useAppStore()
    const workspace = (await import('./workspace')).useWorkspaceStore()
    app.shell = 'workspace'
    workspace.workspaceId = 'workspace-a'
    workspaceOpen.mockRejectedValueOnce(new Error('open failed'))

    await expect(app.switchWorkspace('workspace-b')).resolves.toBe(false)

    expect(app.shell).toBe('launcher')
    expect(workspace.workspaceId).toBeNull()
    expect(app.lastError?.message).toBe('open failed')
  })
})
