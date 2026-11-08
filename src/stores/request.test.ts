import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'
import { ipc, type OAuth2TokenResponse, type CancelledEvent, type ErrorEvent } from '@/ipc'
import { createPinia, setActivePinia } from 'pinia'

const { sendRequest, oauth2FetchToken, cancelOAuth2Token, cancelRequest, historyRecordError, callbacks } = vi.hoisted(() => ({
  callbacks: { current: null as { onCancelled?: (event: CancelledEvent) => void; onError?: (event: ErrorEvent) => void } | null },
  historyRecordError: vi.fn(), sendRequest: vi.fn(), oauth2FetchToken: vi.fn(), cancelOAuth2Token: vi.fn(), cancelRequest: vi.fn(),
}))

vi.mock('vue', async importOriginal => ({
  ...await importOriginal<typeof import('vue')>(),
  onMounted: (callback: () => void) => callback(), onUnmounted: () => {},
}))
vi.mock('@/ipc', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/ipc')>()
  const { needsTokenRefresh, refreshGrant, applyTokenResponse } = await import('@/features/oauth2Lifecycle')
  return ({
  inTauri: true,
  toHistoryRequestPreview: actual.toHistoryRequestPreview,
  ipc: { sendRequest, oauth2FetchToken, historyRecordError, cancelOAuth2Token, cancelRequest,
    oauth2AuthOperation: async (input: { auth: import('@/ipc').AuthConfig; operationId: string; nodeId?: string; environmentId?: string; settings: import('@/ipc').RequestSettings; ephemeral?: boolean }, session: unknown) => {
      if (!needsTokenRefresh(input.auth)) return { auth: input.auth }
      const token = await oauth2FetchToken({ operationId: input.operationId, nodeId: input.nodeId, environmentId: input.environmentId,
        request: input.auth.tokenRequest, grant: refreshGrant(input.auth), settings: input.settings }, session)
      return token ? { auth: applyTokenResponse(input.auth, token, Date.now()), token, sourceId: input.ephemeral ? undefined : input.nodeId } : null
    },
  },
  normalizeError: (error: unknown) => ({
    kind: 'io',
    message: error instanceof Error ? error.message : String(error),
    hints: [],
  }),
  onRequestEvents: (handlers: typeof callbacks.current) => { callbacks.current = handlers; return () => {} },
})})

import { useRequest } from './request'
import { useTabsStore } from './tabs'
import { useWorkspaceStore } from './workspace'

describe('request execution guard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    sendRequest.mockReset()
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('does not invoke IPC when a history preview is sent through the store entry point', async () => {
    // `useRequest` installs lifecycle listeners in application setup. The guard is deliberately
    // tested without mounting because it must run before any workspace/IPC work begins.
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const tabs = useTabsStore()
    const preview = tabs.open({
      title: '历史请求',
      url: 'https://api.test/history',
      historyPreview: true,
    })
    const workspace = useWorkspaceStore()
    workspace.ready = true
    workspace.workspaceId = 'workspace-id'
    workspace.sessionId = 'session-id'

    await useRequest().send()

    expect(preview.historyPreview).toBe(true)
    expect(sendRequest).not.toHaveBeenCalled()
  })
})

describe('OAuth2 send preflight', () => {
  const scopes: ReturnType<typeof effectScope>[] = []
  const requests: ReturnType<typeof useRequest>[] = []
  function setup() {
    const tabs = useTabsStore(), workspace = useWorkspaceStore()
    workspace.ready = true; workspace.workspaceId = 'workspace-a'; workspace.sessionId = 'session-a'
    const tab = tabs.open({ title: 'POST test', method: 'POST', url: 'https://api.test/write',
      auth: { type: 'oauth2', accessToken: 'old', headerPrefix: 'Bearer', autoRefresh: true, obtainedAt: 1000, expiresAt: 2000,
        tokenRequest: { tokenUrl: 'https://auth.test/token', clientId: 'id', clientSecret: 'secret', scope: '', refreshToken: 'refresh-old', clientAuth: 'basic' } } })
    vi.spyOn(workspace, 'saveDirtyEnvironments').mockResolvedValue(true)
    const scope = effectScope(); scopes.push(scope)
    const request = scope.run(() => useRequest())!; requests.push(request)
    return { tabs, workspace, tab, request }
  }
  function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r }); return { resolve, promise } }
  async function flush() { for (let i = 0; i < 8; i++) await Promise.resolve() }
  beforeEach(() => {
    setActivePinia(createPinia()); vi.useFakeTimers(); vi.setSystemTime(10000)
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    historyRecordError.mockReset().mockResolvedValue(undefined)
    sendRequest.mockReset().mockResolvedValue({ id: 'handle' })
    oauth2FetchToken.mockReset().mockResolvedValue({ accessToken: 'new', refreshToken: 'refresh-new', expiresIn: 3600 })
    cancelOAuth2Token.mockReset().mockResolvedValue(true); cancelRequest.mockReset().mockResolvedValue(true)
  })
  afterEach(async () => { for (const request of requests.splice(0)) request.cancel(); for (const scope of scopes.splice(0)) scope.stop(); await flush(); vi.restoreAllMocks(); vi.useRealTimers() })

  it('refreshes before POST, patches rotation, and sends exactly once', async () => {
    const { tabs, request } = setup(); await request.send()
    expect(oauth2FetchToken).toHaveBeenCalledOnce()
    expect(oauth2FetchToken).toHaveBeenCalledWith(expect.objectContaining({ grant: 'refreshToken' }), { workspaceId: 'workspace-a', sessionId: 'session-a' })
    expect(tabs.active!.auth).toMatchObject({ accessToken: 'new', obtainedAt: 10000, expiresAt: 3610000, tokenRequest: { refreshToken: 'refresh-new' } })
    expect(sendRequest).toHaveBeenCalledOnce(); expect(sendRequest.mock.calls[0][0].auth.accessToken).toBe('new')
    expect(tabs.active!.actualRequest).toBeNull()
  })
  it('fails closed on provider failure without replacing credentials', async () => {
    const { tabs, request } = setup()
    oauth2FetchToken.mockRejectedValue({ kind: 'http', message: 'failed', hints: [] })
    await request.send()
    expect(sendRequest).not.toHaveBeenCalled(); expect(tabs.active!.auth).toMatchObject({ accessToken: 'old' })
    expect(tabs.active!.error).not.toBeNull(); expect(request.sending.value).toBe(false)
  })
  it('retains refresh rotation even when returned lifetime is too short to send', async () => {
    const { tabs, request } = setup()
    oauth2FetchToken.mockResolvedValue({ accessToken: 'new', refreshToken: 'rotated', expiresIn: 0 })
    await request.send(); expect(sendRequest).not.toHaveBeenCalled()
    expect(tabs.active!.auth).toMatchObject({ accessToken: 'new', tokenRequest: { refreshToken: 'rotated' } })
    expect(tabs.active!.error).not.toBeNull()
  })
  it.each(['edit', 'environment', 'session', 'close'] as const)('discards late results after %s', async change => {
    const { tabs, workspace, tab, request } = setup()
    const pending = deferred<OAuth2TokenResponse | null>(); oauth2FetchToken.mockReturnValue(pending.promise)
    const sending = request.send(); await flush(); expect(oauth2FetchToken).toHaveBeenCalledOnce()
    if (change === 'edit') tabs.patch(tab.id, { auth: { type: 'bearer', token: 'user-edit' } })
    if (change === 'environment') workspace.activeEnvironmentId = 'other-env'
    if (change === 'session') workspace.sessionId = 'session-b'
    if (change === 'close') tabs.close(tab.id)
    pending.resolve({ accessToken: 'late', refreshToken: 'late-refresh', expiresIn: 60 }); await sending
    expect(sendRequest).not.toHaveBeenCalled(); expect(request.sending.value).toBe(false)
    expect(cancelOAuth2Token).toHaveBeenCalledOnce()
    if (change === 'edit') expect(tabs.active!.auth).toEqual({ type: 'bearer', token: 'user-edit' })
  })
  it('cancel then immediately resend is not reset by the first late result', async () => {
    const { tabs, request } = setup()
    const pending = deferred<OAuth2TokenResponse | null>(); oauth2FetchToken.mockReturnValueOnce(pending.promise)
    const first = request.send(); await flush(); request.cancel()
    await request.send(); expect(request.sending.value).toBe(true)
    pending.resolve({ accessToken: 'late-old-operation' }); await first; await flush()
    expect(tabs.active!.auth).toMatchObject({ accessToken: 'new' })
    expect(request.sending.value).toBe(true); expect(sendRequest).toHaveBeenCalledOnce()
  })

  it('skips refresh for unknown, disabled, inherited or explicitly overridden auth', async () => {
    const { tabs, request } = setup(); const initial = tabs.active!.auth
    if (initial?.type !== 'oauth2') throw new Error('fixture')
    for (const auth of [{ ...initial, expiresAt: undefined }, { ...initial, autoRefresh: false }, { type: 'inherit' as const }, initial]) {
      tabs.patch(tabs.active!.id, { auth, headers: auth === initial ? [{ key: 'Authorization', value: 'custom', enabled: true }] : [] })
      await request.send(); request.cancel()
    }
    expect(oauth2FetchToken).not.toHaveBeenCalled(); expect(sendRequest).toHaveBeenCalledTimes(4)
  })
  it('saves rotation before business HTTP and stops on a save failure', async () => {
    const { tabs, request } = setup()
    tabs.patch(tabs.active!.id, { nodeId: 'saved-node' })
    const save = vi.spyOn(tabs, 'save').mockResolvedValueOnce({ ok: true, error: null }).mockResolvedValue({ ok: false, error: { kind: 'io', message: 'write failed', hints: [] } })
    await request.send()
    expect(save).toHaveBeenCalledTimes(2); expect(sendRequest).not.toHaveBeenCalled()
    expect(tabs.active!.auth).toMatchObject({ tokenRequest: { refreshToken: 'refresh-new' } })
    expect(tabs.active!.dirty).toBe(true); expect(tabs.active!.error).not.toBeNull()
    tabs.resetWorkspace()
  })
  it('does not send or write old errors if a session changes during environment save', async () => {
    const { tabs, workspace, request } = setup()
    vi.mocked(workspace.saveDirtyEnvironments).mockImplementation(async () => { workspace.sessionId = 'other'; return false })
    await request.send()
    expect(oauth2FetchToken).not.toHaveBeenCalled(); expect(sendRequest).not.toHaveBeenCalled()
    expect(tabs.active!.error).toBeNull(); expect(request.sending.value).toBe(false)
  })
  it('applies terminal events that arrive before the HTTP handle', async () => {
    const { request } = setup()
    sendRequest.mockImplementation(async () => {
      callbacks.current!.onCancelled!({ id: 'early', workspaceId: 'workspace-a', sessionId: 'session-a' })
      return { id: 'early' }
    })
    await request.send(); expect(request.sending.value).toBe(false)
  })
  it('a late cancelled or wrong-session event cannot reset a newer HTTP request', async () => {
    const { request } = setup()
    sendRequest.mockResolvedValueOnce({ id: 'old-handle' }).mockResolvedValueOnce({ id: 'new-handle' })
    await request.send(); request.cancel(); await request.send()
    callbacks.current!.onCancelled!({ id: 'old-handle', workspaceId: 'workspace-a', sessionId: 'session-a' })
    callbacks.current!.onCancelled!({ id: 'new-handle', workspaceId: 'workspace-a', sessionId: 'old-session' })
    expect(request.sending.value).toBe(true)
    callbacks.current!.onCancelled!({ id: 'new-handle', workspaceId: 'workspace-a', sessionId: 'session-a' })
    expect(request.sending.value).toBe(false)
  })
  it('uses inherited source credentials for this send without copying them into the child', async () => {
    const { tabs, request } = setup()
    tabs.patch(tabs.active!.id, { auth: { type: 'inherit' }, sourceRequestId: 'source-request' })
    const inherited = { type: 'oauth2' as const, accessToken: 'parent-token', headerPrefix: 'Bearer', autoRefresh: true, expiresAt: 999999 }
    const prepare = vi.spyOn(ipc, 'oauth2AuthOperation').mockResolvedValue({ auth: inherited, sourceId: 'folder', token: { accessToken: 'parent-token', expiresIn: 60 } })
    await request.send()
    expect(prepare).toHaveBeenCalledWith(expect.objectContaining({ auth: { type: 'inherit' }, nodeId: 'source-request' }), { workspaceId: 'workspace-a', sessionId: 'session-a' })
    expect(tabs.active!.auth).toEqual({ type: 'inherit' })
    expect(sendRequest.mock.calls[0][0].auth).toEqual({ type: 'inherit' })
    expect(sendRequest.mock.calls[0][1]).toMatchObject({ ephemeral: true, requestNodeId: 'source-request' })
  })
  it('an inherited preparation failure stops HTTP and shows the fixed safe source hint', async () => {
    const { tabs, request } = setup(); tabs.patch(tabs.active!.id, { auth: { type: 'inherit' } })
    vi.spyOn(ipc, 'oauth2AuthOperation').mockRejectedValue({ kind: 'io', message: 'source changed', code: 'OAUTH2_SOURCE_STALE', hints: [] })
    await request.send(); expect(sendRequest).not.toHaveBeenCalled(); expect(tabs.active!.error).not.toBeNull()
  })
  it('does not silently authorize PKCE without a refresh token', async () => {
    const { tabs, request } = setup(); const auth = tabs.active!.auth
    if (auth?.type !== 'oauth2') throw new Error('fixture')
    tabs.patch(tabs.active!.id, { auth: { ...auth, tokenRequest: { ...auth.tokenRequest!, grantType: 'authorizationCode', refreshToken: '' } } })
    await request.send(); expect(oauth2FetchToken).not.toHaveBeenCalled(); expect(sendRequest).not.toHaveBeenCalled()
    expect(tabs.active!.error).not.toBeNull()
  })
  it('does not record backend Auth/preparation failures as business request history', async () => {
    for (const code of ['OAUTH2_SOURCE_STALE', 'REQUEST_PREPARATION_SOURCE_STALE']) {
      const { tabs, request } = setup()
      sendRequest.mockImplementation(async () => {
        callbacks.current!.onError!({ id: 'early-error', workspaceId: 'workspace-a', sessionId: 'session-a',
          error: { kind: 'io', code, message: 'generic failure', hints: [] } })
        return { id: 'early-error' }
      })
      await request.send()
      expect(request.sending.value).toBe(false)
      expect(tabs.active!.error?.code).toBe(code)
      expect(tabs.active!.actualRequest).toBeNull()
      expect(historyRecordError).not.toHaveBeenCalled()
    }
  })
  it('still records an ordinary transport failure after backend preparation', async () => {
    const { request } = setup(); await request.send()
    callbacks.current!.onError!({ id: 'handle', workspaceId: 'workspace-a', sessionId: 'session-a',
      error: { kind: 'connect', message: 'connection failed', hints: [] } })
    expect(historyRecordError).toHaveBeenCalledOnce()
    expect(request.sending.value).toBe(false)
  })

})
