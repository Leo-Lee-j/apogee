import { describe, expect, it, vi } from 'vitest'
import { applyTokenResponse, createOAuth2RefreshCoordinator, needsTokenRefresh, refreshGrant, tokenState, type OAuth2Auth, type RefreshOwner } from './oauth2Lifecycle'
import type { OAuth2TokenResponse } from '@/ipc'
const auth: OAuth2Auth = { type: 'oauth2', accessToken: 'old', headerPrefix: 'Bearer', autoRefresh: true, obtainedAt: 1000, expiresAt: 2000,
  tokenRequest: { tokenUrl: 'https://auth.test/token', clientId: 'client', clientSecret: 'secret', refreshToken: 'refresh-old', scope: '', clientAuth: 'basic' } }
const owner: RefreshOwner = { workspaceId: 'a', sessionId: 'a1', sourceId: 'request-a', environmentId: 'env-a', environment: {}, settings: {} }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r }); return { promise, resolve } }
const signal = () => new AbortController().signal

describe('OAuth2 expiry', () => {
  it('distinguishes expiry states and applies opt-in', () => {
    expect(tokenState({ ...auth, accessToken: '' }, 1000)).toBe('missing')
    expect(tokenState({ ...auth, expiresAt: undefined }, 1000)).toBe('unknown')
    expect(tokenState({ ...auth, expiresAt: 40001 }, 10000)).toBe('valid')
    expect(tokenState({ ...auth, expiresAt: 40000 }, 10000)).toBe('expiring')
    expect(tokenState(auth, 2000)).toBe('expired')
    expect(tokenState(auth, 999)).toBe('clockInvalid')
    expect(needsTokenRefresh({ ...auth, autoRefresh: false }, 2000)).toBe(false)
    expect(needsTokenRefresh({ ...auth, expiresAt: undefined }, 2000)).toBe(false)
    expect(needsTokenRefresh(auth, 2000)).toBe(true)
  })
  it('uses acquisition start, rotates credentials and clears invalid expiry', () => {
    expect(applyTokenResponse(auth, { accessToken: 'new', refreshToken: 'rotated', expiresIn: 60 }, 4000))
      .toMatchObject({ obtainedAt: 4000, expiresAt: 64000, tokenRequest: { refreshToken: 'rotated' } })
    for (const expiresIn of [undefined, -1, Infinity, NaN, Number.MAX_SAFE_INTEGER, 0.5])
      expect(applyTokenResponse(auth, { accessToken: 'new', expiresIn }, 4000)).not.toHaveProperty('expiresAt')
    expect(applyTokenResponse(auth, { accessToken: 'new', expiresIn: 0 }, 4000).expiresAt).toBe(4000)
    expect(applyTokenResponse(auth, { accessToken: 'new' }, 4000).tokenRequest!.refreshToken).toBe('refresh-old')
  })
})

describe('refresh sharing', () => {
  it('chooses refresh/client credentials and never opens a PKCE browser', () => {
    expect(refreshGrant(auth)).toBe('refreshToken')
    const noRefresh = { ...auth, tokenRequest: { ...auth.tokenRequest!, refreshToken: '' } }
    expect(refreshGrant(noRefresh)).toBe('clientCredentials')
    expect(() => refreshGrant({ ...noRefresh, tokenRequest: { ...noRefresh.tokenRequest, grantType: 'authorizationCode' } })).toThrow()
    expect(() => refreshGrant({ ...auth, tokenRequest: undefined })).toThrow()
  })
  it('deduplicates without letting one waiter cancel another', async () => {
    const coordinator = createOAuth2RefreshCoordinator(8, () => 5000)
    const pending = deferred<OAuth2TokenResponse | null>()
    let transport!: AbortSignal
    const start = vi.fn((_grant, s: AbortSignal) => { transport = s; return pending.promise })
    const a = new AbortController(), b = new AbortController()
    const first = coordinator.refresh(owner, auth, start, a.signal)
    const second = coordinator.refresh({ ...owner }, { ...auth }, start, b.signal)
    await Promise.resolve(); a.abort()
    expect(await first).toBeNull(); expect(transport.aborted).toBe(false)
    pending.resolve({ accessToken: 'new', refreshToken: 'rotated', expiresIn: 60 })
    expect(await second).toMatchObject({ accessToken: 'new', expiresAt: 65000 })
    expect(start).toHaveBeenCalledOnce(); expect(transport.aborted).toBe(false)
  })
  it('aborts only when all waiters leave and discards late success', async () => {
    const coordinator = createOAuth2RefreshCoordinator()
    const pending = deferred<OAuth2TokenResponse | null>()
    let transport!: AbortSignal
    const start = vi.fn((_grant, s: AbortSignal) => { transport = s; return pending.promise })
    const a = new AbortController(), b = new AbortController()
    const first = coordinator.refresh(owner, auth, start, a.signal)
    const second = coordinator.refresh(owner, auth, start, b.signal)
    await Promise.resolve(); a.abort(); b.abort()
    expect(await first).toBeNull(); expect(await second).toBeNull(); expect(transport.aborted).toBe(true)
    pending.resolve({ accessToken: 'late-secret' }); await Promise.resolve()
  })
  it('isolates sessions, sources, environment revisions, settings and edited credentials', async () => {
    const coordinator = createOAuth2RefreshCoordinator()
    const pending = deferred<OAuth2TokenResponse | null>(), start = vi.fn(() => pending.promise)
    const owners = [owner, { ...owner, sessionId: 'a2' }, { ...owner, sourceId: 'request-b' },
      { ...owner, environment: {} }, { ...owner, environmentId: 'env-b' }, { ...owner, settings: { verifyTls: false } }]
    const promises = owners.map(item => coordinator.refresh(item, auth, start, signal()))
    promises.push(coordinator.refresh(owner, { ...auth, accessToken: 'edited' }, start, signal()))
    await Promise.resolve(); expect(start).toHaveBeenCalledTimes(7)
    pending.resolve({ accessToken: 'new' }); await Promise.all(promises)
  })
})

it('bounds in-flight work, cleans failures and retains no settled cache', async () => {
  const coordinator = createOAuth2RefreshCoordinator(1)
  await expect(coordinator.refresh(owner, auth, vi.fn().mockRejectedValue({ kind: 'http' }), signal())).rejects.toMatchObject({ kind: 'http' })
  const pending = deferred<OAuth2TokenResponse | null>(), start = vi.fn(() => pending.promise)
  const first = coordinator.refresh(owner, auth, start, signal())
  await expect(coordinator.refresh({ ...owner, sourceId: 'other' }, auth, start, signal())).rejects.toMatchObject({ kind: 'http' })
  const cancelled = new AbortController(); cancelled.abort()
  expect(await coordinator.refresh(owner, auth, start, cancelled.signal)).toBeNull()
  pending.resolve(null); expect(await first).toBeNull()
  expect(start).toHaveBeenCalledOnce()
  const success = vi.fn().mockResolvedValue({ accessToken: 'new' })
  await coordinator.refresh(owner, auth, success, signal()); await coordinator.refresh(owner, auth, success, signal())
  expect(success).toHaveBeenCalledTimes(2)
})

it('uses host exchange start rather than counting the PKCE browser wait', () => {
  const next = applyTokenResponse(auth, { accessToken: 'pkce', obtainedAt: 181000, expiresIn: 60 }, 1000)
  expect(next).toMatchObject({ obtainedAt: 181000, expiresAt: 241000 })
  expect(tokenState(next, 182000)).toBe('valid')
})
