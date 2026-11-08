import type { AppError, AuthConfig, OAuth2Grant, OAuth2TokenResponse, RequestSettings, WorkspaceSession } from '@/ipc'

export type OAuth2Auth = Extract<AuthConfig, { type: 'oauth2' }>
export const EXPIRY_SKEW_MS = 30_000
const MAX_DATE_MS = 8_640_000_000_000_000
export type TokenState = 'missing' | 'unknown' | 'valid' | 'expiring' | 'expired' | 'clockInvalid'

/** Only provider expires_in is used. Pasted tokens/JWT payloads are not validity evidence. */
export function tokenState(auth: OAuth2Auth, now = Date.now()): TokenState {
  if (!auth.accessToken.trim()) return 'missing'
  if (auth.obtainedAt != null && now < auth.obtainedAt) return 'clockInvalid'
  if (auth.expiresAt == null || !Number.isSafeInteger(auth.expiresAt) || auth.expiresAt < 0
    || auth.expiresAt > MAX_DATE_MS) return 'unknown'
  if (now >= auth.expiresAt) return 'expired'
  return now + EXPIRY_SKEW_MS >= auth.expiresAt ? 'expiring' : 'valid'
}

/** Bound the absolute date and use operation START time, not receipt time, conservatively. */
export function applyTokenResponse(auth: OAuth2Auth, response: OAuth2TokenResponse, startedAt: number): OAuth2Auth {
  const { obtainedAt: _oldStart, expiresAt: _oldExpiry, tokenContext: _oldContext, ...base } = auth
  if (response.obtainedAt != null && Number.isSafeInteger(response.obtainedAt)
    && response.obtainedAt >= 0 && response.obtainedAt <= MAX_DATE_MS) startedAt = response.obtainedAt
  const lifetime = response.expiresIn
  const expiry = lifetime == null ? undefined : startedAt + lifetime * 1000
  const known = lifetime != null && Number.isSafeInteger(lifetime) && lifetime >= 0
    && Number.isSafeInteger(expiry) && expiry! <= MAX_DATE_MS && expiry! >= startedAt
  return { ...base, accessToken: response.accessToken, obtainedAt: startedAt,
    ...(known ? { expiresAt: expiry } : {}),
    ...(response.tokenContext ? { tokenContext: response.tokenContext } : {}),
    ...(auth.tokenRequest ? { tokenRequest: { ...auth.tokenRequest,
      refreshToken: response.refreshToken ?? auth.tokenRequest.refreshToken } } : {}) }
}

export function needsTokenRefresh(auth: AuthConfig | undefined, now = Date.now()): auth is OAuth2Auth {
  return auth?.type === 'oauth2' && auth.autoRefresh === true && !['valid', 'unknown'].includes(tokenState(auth, now))
}

export function refreshGrant(auth: OAuth2Auth): OAuth2Grant {
  if (!auth.tokenRequest) throw lifecycleError('请先配置 Token 获取，或关闭发送前自动刷新')
  if (auth.tokenRequest.refreshToken.trim()) return 'refreshToken'
  if ((auth.tokenRequest.grantType ?? 'clientCredentials') === 'clientCredentials') return 'clientCredentials'
  // Never open a browser silently during Send, nor replay a write request after 401.
  throw lifecycleError('授权码 Token 无 Refresh Token，请在 Auth 中重新进行浏览器授权')
}
export function lifecycleError(label: string): AppError {
  return { kind: 'http', message: 'OAuth2 发送前刷新未完成，请求未发送', hints: [{ label }] }
}

export interface RefreshOwner extends WorkspaceSession {
  sourceId: string
  environmentId?: string
  /** A captured environment revision/object identity, never persisted or displayed. */
  environment: unknown
  settings: RequestSettings
}
interface Entry {
  owner: RefreshOwner
  fingerprint: string
  controller: AbortController
  waiters: number
  settled: boolean
  promise: Promise<OAuth2Auth | null>
}
/** In-flight only, capped; no settled credential cache and no token-bearing diagnostic keys. */
export function createOAuth2RefreshCoordinator(limit = 8, now: () => number = () => Date.now()) {
  const entries: Entry[] = []
  function sameOwner(a: RefreshOwner, b: RefreshOwner) {
    return a.workspaceId === b.workspaceId && a.sessionId === b.sessionId && a.sourceId === b.sourceId
      && a.environmentId === b.environmentId && a.environment === b.environment
      && JSON.stringify(a.settings) === JSON.stringify(b.settings)
  }
  return {
    refresh(owner: RefreshOwner, auth: OAuth2Auth,
      start: (grant: OAuth2Grant, signal: AbortSignal) => Promise<OAuth2TokenResponse | null>, signal: AbortSignal): Promise<OAuth2Auth | null> {
      if (signal.aborted) return Promise.resolve(null)
      // Sensitive equality material exists only while this operation is in flight; never log it.
      const fingerprint = JSON.stringify(auth)
      let entry = entries.find(item => !item.controller.signal.aborted && item.fingerprint === fingerprint && sameOwner(item.owner, owner))
      if (!entry) {
        if (entries.length >= limit) return Promise.reject(lifecycleError('同时刷新数量达到上限，请稍后重试'))
        const grant = refreshGrant(auth)
        const controller = new AbortController()
        const startedAt = now()
        const created: Entry = { owner, fingerprint, controller, waiters: 0, settled: false, promise: Promise.resolve(null) }
        created.promise = Promise.resolve().then(() => controller.signal.aborted ? null : start(grant, controller.signal))
          .then(response => response && !controller.signal.aborted ? applyTokenResponse(auth, response, startedAt) : null)
          .finally(() => { created.settled = true; const index = entries.indexOf(created); if (index >= 0) entries.splice(index, 1) })
        entries.push(created)
        entry = created
      }
      const shared = entry
      shared.waiters++
      return new Promise((resolve, reject) => {
        let finished = false
        const finish = (value: OAuth2Auth | null, error?: unknown) => {
          if (finished) return
          finished = true
          signal.removeEventListener('abort', abort)
          shared.waiters--
          if (shared.waiters === 0 && !shared.settled) shared.controller.abort()
          if (error !== undefined) reject(error); else resolve(value)
        }
        const abort = () => finish(null)
        signal.addEventListener('abort', abort, { once: true })
        if (signal.aborted) abort()
        shared.promise.then(value => finish(value), error => finish(null, error))
      })
    },
  }
}
