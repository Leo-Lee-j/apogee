import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ipc } from '@/ipc'
import { useOAuth2TokenActions } from './oauth2TokenOperation'
const { workspace } = vi.hoisted(() => ({ workspace: { workspaceId: 'workspace-a', sessionId: 'session-a', activeEnvironmentId: 'environment-a' as string | null,
  saveDirtyEnvironments: vi.fn(), lastError: null } }))
vi.mock('@/stores/workspace', () => ({ useWorkspaceStore: () => workspace }))
const request = { tokenUrl: 'https://auth.test/token', clientId: 'id', clientSecret: 'secret', refreshToken: '', clientAuth: 'basic' as const, scope: '' }
beforeEach(() => { vi.restoreAllMocks(); workspace.workspaceId = 'workspace-a'; workspace.sessionId = 'session-a'; workspace.activeEnvironmentId = 'environment-a'; workspace.saveDirtyEnvironments.mockReset(); workspace.saveDirtyEnvironments.mockResolvedValue(true) })
describe('OAuth2 operation ownership', () => {
  it('flushes variables and sends the captured session/environment with network settings', async () => {
    const fetch = vi.spyOn(ipc, 'oauth2FetchToken').mockResolvedValue({ accessToken: 'token' })
    const action = useOAuth2TokenActions()('node', { timeoutMs: 1000 }, () => true)
    expect(await action(request, 'clientCredentials', 'operation')).toEqual({ accessToken: 'token' })
    expect(workspace.saveDirtyEnvironments).toHaveBeenCalledOnce()
    expect(fetch).toHaveBeenCalledWith(expect.objectContaining({ environmentId: 'environment-a', nodeId: 'node', settings: { timeoutMs: 1000 } }), { workspaceId: 'workspace-a', sessionId: 'session-a' })
  })
  it('does not send after workspace or environment changes during a variable flush', async () => {
    const fetch = vi.spyOn(ipc, 'oauth2FetchToken').mockResolvedValue(null)
    workspace.saveDirtyEnvironments.mockImplementation(async () => { workspace.sessionId = 'session-b'; return true })
    const action = useOAuth2TokenActions()(undefined, {}, () => true)
    await expect(action(request, 'clientCredentials', 'op')).rejects.toMatchObject({ kind: 'io' })
    expect(fetch).not.toHaveBeenCalled()
  })
  it('discards even a successful token returned after environment changes', async () => {
    vi.spyOn(ipc, 'oauth2FetchToken').mockImplementation(async () => { workspace.activeEnvironmentId = 'environment-b'; return { accessToken: 'private-token' } })
    const action = useOAuth2TokenActions()('node', {}, () => true)
    await expect(action(request, 'clientCredentials', 'op')).rejects.toMatchObject({ kind: 'io' })
  })
})

describe('OAuth2 preflight cancellation', () => {
  it('never starts IPC when cancelled before acquisition or during environment flush', async () => {
    const fetch = vi.spyOn(ipc, 'oauth2FetchToken').mockResolvedValue(null)
    const controller = new AbortController()
    const action = useOAuth2TokenActions()('node', {}, () => true)
    workspace.saveDirtyEnvironments.mockImplementation(async () => { controller.abort(); return true })
    expect(await action(request, 'clientCredentials', 'op', controller.signal)).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
    workspace.saveDirtyEnvironments.mockClear()
    expect(await action(request, 'clientCredentials', 'op', controller.signal)).toBeNull()
    expect(workspace.saveDirtyEnvironments).not.toHaveBeenCalled()
  })
  it('drops successful IPC credentials after explicit cancellation', async () => {
    const controller = new AbortController()
    vi.spyOn(ipc, 'oauth2FetchToken').mockImplementation(async () => { controller.abort(); return { accessToken: 'late-token' } })
    const action = useOAuth2TokenActions()('node', {}, () => true)
    expect(await action(request, 'clientCredentials', 'op', controller.signal)).toBeNull()
  })
})

it('manual Auth operations use the same source-aware IPC with owner and credential context', async () => {
  const token = { accessToken: 'new', tokenContext: 'private-context' }
  const auth = { type: 'oauth2' as const, accessToken: 'old', headerPrefix: 'Bearer', tokenRequest: request }
  const fetch = vi.spyOn(ipc, 'oauth2AuthOperation').mockResolvedValue({ auth, sourceId: 'folder', token })
  const before = vi.fn().mockResolvedValue(true)
  const action = useOAuth2TokenActions()('folder', {}, () => true, 'folder', false, before)
  expect(await action(request, 'refreshToken', 'operation', undefined, auth)).toEqual(token)
  expect(before).toHaveBeenCalledOnce()
  expect(fetch).toHaveBeenCalledWith(expect.objectContaining({ ownerId: 'folder', auth, grant: 'refreshToken', ephemeral: false }), { workspaceId: 'workspace-a', sessionId: 'session-a' })
})
it('does not start a credential operation when pending request save fails', async () => {
  const fetch = vi.spyOn(ipc, 'oauth2AuthOperation')
  const action = useOAuth2TokenActions()('node', {}, () => true, 'node', false, async () => false)
  await expect(action(request, 'clientCredentials', 'op', undefined, { type: 'oauth2', accessToken: '', headerPrefix: 'Bearer' })).rejects.toMatchObject({ kind: 'io' })
  expect(fetch).not.toHaveBeenCalled()
})

it('captures the current persisted Auth baseline after save preflight, not before it', async () => {
  const old = { type: 'oauth2' as const, accessToken: 'old', headerPrefix: 'Bearer' }
  const saved = { ...old, accessToken: 'new' }
  let baseline = old
  const fetch = vi.spyOn(ipc, 'oauth2AuthOperation').mockResolvedValue({ auth: saved, token: { accessToken: 'new' } })
  const action = useOAuth2TokenActions()('node', {}, () => true, 'node', false,
    async () => { baseline = saved; return true }, () => baseline)
  await action(request, 'refreshToken', 'operation', undefined, saved)
  expect(fetch).toHaveBeenCalledWith(expect.objectContaining({ expectedAuth: saved }), expect.anything())
})
