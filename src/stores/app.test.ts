import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const workspaceClose = vi.fn()
const workspaceOpen = vi.fn()
const workspaceList = vi.fn()
const appSettingsLoad = vi.fn()
const appRestart = vi.fn()
const updateInstallAndRestart = vi.fn()

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: new Proxy({}, {
    get(_target, key: string) {
      if (key === 'workspaceClose') return workspaceClose
      if (key === 'workspaceOpen') return workspaceOpen
      if (key === 'workspaceList') return workspaceList
      if (key === 'appSettingsLoad') return appSettingsLoad
      if (key === 'appRestart') return appRestart
      if (key === 'updateInstallAndRestart') return updateInstallAndRestart
      if (key === 'workspaceReadRecovery') return vi.fn().mockResolvedValue({ requests: [], containers: [] })
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
    appRestart.mockReset().mockResolvedValue(undefined)
    updateInstallAndRestart.mockReset().mockResolvedValue(undefined)
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

  it('cancels restart when editors cannot be saved and restores the UI on restart failure', async () => {
    const app = useAppStore()
    app.shell = 'workspace'
    const tabs = (await import('./tabs')).useTabsStore()
    vi.spyOn(tabs, 'flushAll').mockResolvedValueOnce(false)
    expect(await app.restart()).toBe(false)
    expect(appRestart).not.toHaveBeenCalled()
    expect(app.shell).toBe('workspace')
    appRestart.mockRejectedValueOnce(new Error('restart failed'))
    expect(await app.restart()).toBe(false)
    expect(app.shell).toBe('workspace')
    expect(app.lastError?.message).toBe('restart failed')
  })

  it('keeps the workspace open when Rust rejects close', async () => {
    workspaceClose.mockRejectedValueOnce(new Error('close failed'))
    const app = useAppStore()
    app.shell = 'workspace'

    await expect(app.closeWorkspace()).resolves.toBe(false)

    expect(app.shell).toBe('workspace')
    expect(app.lastError?.message).toBe('close failed')
  })

  it('waits for workspace and editor saves before handing a verified update to the installer', async () => {
    const app = useAppStore()
    app.shell = 'workspace'
    const workspace = (await import('./workspace')).useWorkspaceStore()
    const tabs = (await import('./tabs')).useTabsStore()
    const recovery = (await import('./editorRecovery')).useEditorRecoveryStore()
    const flush = vi.spyOn(workspace, 'flushWorkspaceState').mockResolvedValueOnce(true)
    const environments = vi.spyOn(workspace, 'saveDirtyEnvironments').mockResolvedValueOnce(true)
    const requests = vi.spyOn(tabs, 'flushAll').mockResolvedValueOnce(true)
    const drafts = vi.spyOn(recovery, 'prepareLeave').mockResolvedValueOnce(true)

    expect(await app.installUpdateAndRestart('v0.2.0')).toBe(true)
    expect(updateInstallAndRestart).toHaveBeenCalledWith('v0.2.0')
    expect(app.shell).toBe('closing')
    expect(flush.mock.invocationCallOrder[0]).toBeLessThan(environments.mock.invocationCallOrder[0]!)
    expect(environments.mock.invocationCallOrder[0]).toBeLessThan(requests.mock.invocationCallOrder[0]!)
    expect(requests.mock.invocationCallOrder[0]).toBeLessThan(drafts.mock.invocationCallOrder[0]!)
    expect(drafts.mock.invocationCallOrder[0]).toBeLessThan(updateInstallAndRestart.mock.invocationCallOrder[0]!)
    expect(appRestart).not.toHaveBeenCalled()
  })

  it('does not start an installer if saving fails or the draft prompt is cancelled', async () => {
    const app = useAppStore()
    app.shell = 'workspace'
    const tabs = (await import('./tabs')).useTabsStore()
    vi.spyOn(tabs, 'flushAll').mockResolvedValueOnce(false)
    expect(await app.installUpdateAndRestart('v0.2.0')).toBe(false)
    expect(app.shell).toBe('workspace')
    expect(app.lastError?.message).toContain('保存请求失败')
    const recovery = (await import('./editorRecovery')).useEditorRecoveryStore()
    vi.spyOn(recovery, 'prepareLeave').mockResolvedValueOnce(false)
    expect(await app.installUpdateAndRestart('v0.2.0')).toBe(false)
    expect(updateInstallAndRestart).not.toHaveBeenCalled()
    expect(app.shell).toBe('workspace')
    expect(app.lastError).toBeNull()
  })

  it.each(['launcher', 'workspace'] as const)('restores the %s if the installer cannot start', async shell => {
    const app = useAppStore()
    app.shell = shell
    updateInstallAndRestart.mockRejectedValueOnce(new Error('安装程序授权已取消'))
    expect(await app.installUpdateAndRestart('v0.2.0')).toBe(false)
    expect(app.shell).toBe(shell)
    expect(app.lastError?.message).toBe('安装程序授权已取消')
    expect(appRestart).not.toHaveBeenCalled()
  })

  it.each(['close', 'switch'] as const)('preserves the workspace and save error when %s cannot flush requests', async (action) => {
    const app = useAppStore()
    const workspace = (await import('./workspace')).useWorkspaceStore()
    const tabs = (await import('./tabs')).useTabsStore()
    app.shell = 'workspace'
    workspace.workspaceId = 'workspace-a'
    workspace.lastError = { kind: 'io', message: 'earlier error', hints: [] }
    tabs.lastError = { kind: 'io', message: 'request is read only', hints: [{ label: '检查目录权限后重试' }] }
    vi.spyOn(tabs, 'flushAll').mockResolvedValueOnce(false)

    const result = action === 'close' ? await app.closeWorkspace() : await app.switchWorkspace('workspace-b')

    expect(result).toBe(false)
    expect(workspaceClose).not.toHaveBeenCalled()
    expect(workspaceOpen).not.toHaveBeenCalled()
    expect(app.shell).toBe('workspace')
    expect(app.lastError?.message).toBe('request is read only')
    expect(app.lastError?.hints).toHaveLength(1)
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
