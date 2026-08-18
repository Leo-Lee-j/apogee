import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const workspaceClose = vi.fn()
const workspaceOpen = vi.fn()
const workspaceList = vi.fn()
const appSettingsLoad = vi.fn()

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: new Proxy({}, {
    get(_target, key: string) {
      if (key === 'workspaceClose') return workspaceClose
      if (key === 'workspaceOpen') return workspaceOpen
      if (key === 'workspaceList') return workspaceList
      if (key === 'appSettingsLoad') return appSettingsLoad
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
    appSettingsLoad.mockReset()
    appSettingsLoad.mockResolvedValue({
      settingsVersion: 1,
      defaultWorkspaceRoot: 'C:/workspaces',
      openLastWorkspaceOnStart: false,
      showMemoryMonitor: true,
      lowMemoryMode: false,
      proxyMode: 'system',
      proxyUrl: '',
      timeoutMs: 30000,
      verifyTls: true,
      themeMode: 'light',
      language: 'system',
      fontFamily: 'system',
      editorFontFamily: 'system',
      uiFontSize: 13,
      resultFontSize: 13,
    })
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

  it('does not begin opening the next workspace until the old session closes', async () => {
    let closeOld!: () => void
    workspaceClose.mockImplementationOnce(() => new Promise<void>((resolve) => { closeOld = resolve }))
    const app = useAppStore()
    const workspace = (await import('./workspace')).useWorkspaceStore()
    app.shell = 'workspace'
    workspace.workspaceId = 'workspace-a'
    workspaceOpen.mockResolvedValueOnce({
      descriptor: { id: 'workspace-b', name: 'B', path: 'C:/b', lastOpenedAt: 2, status: 'available' },
      workspaceId: 'workspace-b', sessionId: 'session-b',
      snapshot: { id: 'workspace-b', name: 'B', nodes: [], environments: [] },
      state: { activeEnvironmentId: null, activeEnvironmentSet: false },
    })

    const switching = app.switchWorkspace('workspace-b')
    await vi.waitFor(() => expect(workspaceClose).toHaveBeenCalledTimes(1))
    expect(app.shell).toBe('closing')
    expect(workspace.workspaceId).toBe('workspace-a')
    expect(workspaceOpen).not.toHaveBeenCalled()

    closeOld()
    await expect(switching).resolves.toBe(true)
    expect(workspace.workspaceId).toBe('workspace-b')
  })

  it('only auto-opens the latest available workspace when the setting is enabled', async () => {
    appSettingsLoad.mockResolvedValueOnce({
      ...(await appSettingsLoad()),
      openLastWorkspaceOnStart: true,
    })
    workspaceList.mockResolvedValueOnce([
      { id: 'missing', name: 'Missing', path: 'C:/missing', lastOpenedAt: 3, status: 'missing' },
      { id: 'workspace-b', name: 'B', path: 'C:/b', lastOpenedAt: 2, status: 'available' },
    ])
    workspaceOpen.mockResolvedValueOnce({
      descriptor: { id: 'workspace-b', name: 'B', path: 'C:/b', lastOpenedAt: 2, status: 'available' },
      workspaceId: 'workspace-b', sessionId: 'session-b',
      snapshot: { id: 'workspace-b', name: 'B', nodes: [], environments: [] },
      state: { activeEnvironmentId: null, activeEnvironmentSet: false },
    })

    const app = useAppStore()
    await app.initialize()

    expect(workspaceOpen).toHaveBeenCalledWith({ workspaceId: 'workspace-b' })
    expect(app.shell).toBe('workspace')
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
