import { describe, expect, it } from 'vitest'
import { AUTH_OPTIONS, authFields, authSummary, postmanAuth, selectAuth, defaultTokenRequest, oauth2OperationError } from './requestAuth'
import { toHistoryRequestPreview, type RequestSpec } from '@/ipc'
import { exportCurl } from './curl'
import { buildRequestDocumentation } from './requestDocumentation'
import type { Tab } from '@/stores/tabs'
const spec = (auth: RequestSpec['auth']): RequestSpec => ({ method: 'GET', url: 'https://example.test/', headers: [], params: [], pathParams: [], body: { type: 'none' }, settings: {}, auth })
describe('request auth models and exports', () => {
  it('defines complete, isolated defaults for every offered method', () => {
    for (const option of AUTH_OPTIONS) {
      const auth = selectAuth(option.value, { type: 'bearer', token: 'previous-secret' })
      expect(auth.type).toBe(option.value)
      expect(selectAuth(option.value, auth)).toBe(auth)
      if (option.value !== 'bearer') expect(JSON.stringify(auth)).not.toContain('previous-secret')
      expect(authSummary(auth)).toBe(option.label)
      for (const field of authFields(auth)) expect(auth).toHaveProperty(field.key)
    }
  })
  it('exports OAuth1, OAuth2 and JWT configuration rather than stale signed tokens', () => {
    const auth = selectAuth('oauth1', { type: 'none' })
    expect(postmanAuth(auth)).toMatchObject({ type: 'oauth1', oauth1: expect.arrayContaining([{ key: 'signatureMethod', value: 'HMAC-SHA1', type: 'string' }]) })
    expect(postmanAuth(selectAuth('oauth2', auth))).toMatchObject({ type: 'oauth2' })
    expect(postmanAuth(selectAuth('jwt', auth))).toMatchObject({ type: 'jwt' })
    expect(postmanAuth({ type: 'inherit' })).toBeUndefined()
    expect(postmanAuth({ type: 'none' })).toEqual({ type: 'noauth' })
  })
  it('never puts new auth credentials or manual Authorization into history or docs', () => {
    for (const type of ['oauth1', 'oauth2', 'jwt'] as const) {
      const auth = selectAuth(type, { type: 'none' })
      for (const field of authFields(auth)) if (field.secret) (auth as unknown as Record<string, string>)[field.key] = 'private-secret'
      const request = { ...spec(auth), headers: [{ key: 'Authorization', value: 'manual-private', enabled: true, kind: 'text' as const }] }
      const history = JSON.stringify(toHistoryRequestPreview(request))
      expect(history).not.toContain('private-secret'); expect(history).not.toContain('manual-private')
      expect(history).not.toContain('"auth"')
      const docs = JSON.stringify(buildRequestDocumentation({ ...request, id: 'id', title: 'Auth', dirty: false } as Tab))
      expect(docs).not.toContain('private-secret'); expect(docs).not.toContain('manual-private')
      expect(docs).toContain(authSummary(auth))
    }
  })
  it('keeps token-fetch credentials out of docs/history and retains them in explicit Postman export', () => {
    const auth = { type: 'oauth2' as const, accessToken: 'access', headerPrefix: 'Bearer', tokenRequest: { ...defaultTokenRequest(), clientSecret: 'private-client', refreshToken: 'private-refresh' } }
    expect(JSON.stringify(toHistoryRequestPreview(spec(auth)))).not.toContain('private-')
    expect(JSON.stringify(buildRequestDocumentation({ ...spec(auth), id: 'id', title: 'Auth', dirty: false } as Tab))).not.toContain('private-')
    const exported = JSON.stringify(postmanAuth(auth)); expect(exported).toContain('private-client'); expect(exported).toContain('client_credentials')
  })
  it('exports OAuth2 curl headers and refuses to silently drop dynamic signed auth', () => {
    expect(exportCurl(spec({ type: 'oauth2', accessToken: 'access', headerPrefix: 'Token' }))).toContain('Authorization: Token access')
    for (const type of ['oauth1', 'jwt'] as const) expect(() => exportCurl(spec(selectAuth(type, { type: 'none' })))).toThrow('签名')
    const request = spec(selectAuth('jwt', { type: 'none' }))
    request.headers = [{ key: 'Authorization', value: 'manual', enabled: true, kind: 'text' }]
    expect(exportCurl(request).match(/Authorization:/g)).toHaveLength(1)
  })
})

it('exports PKCE configuration but never puts browser endpoints or credentials in public previews', () => {
  const auth = { type: 'oauth2' as const, accessToken: 'private-access', headerPrefix: 'Bearer', tokenRequest: { ...defaultTokenRequest(), grantType: 'authorizationCode' as const,
    authorizationUrl: 'https://private-provider.test/authorize', redirectUri: 'http://127.0.0.1:53682/oauth/callback', clientSecret: 'private-client' } }
  const exported = postmanAuth(auth)
  expect(exported).toMatchObject({ oauth2: expect.arrayContaining([
    { key: 'grant_type', value: 'authorization_code_with_pkce', type: 'string' },
    { key: 'code_challenge_method', value: 'S256', type: 'string' },
    { key: 'authUrl', value: auth.tokenRequest.authorizationUrl, type: 'string' },
  ]) })
  expect(JSON.stringify(exported)).not.toContain('code_verifier')
  expect(JSON.stringify(toHistoryRequestPreview(spec(auth)))).not.toContain('private-')
  expect(JSON.stringify(buildRequestDocumentation({ ...spec(auth), id: 'id', title: 'Auth', dirty: false } as Tab))).not.toContain('private-')
})

it('maps only static OAuth error codes without restoring arbitrary provider error details', () => {
  const failure = { kind: 'io' as const, message: '通用失败', hints: [], code: 'OAUTH2_CALLBACK_BIND_FAILED' }
  expect(oauth2OperationError(failure).message).toContain('回调端口无法绑定')
  const unknown = { ...failure, code: 'UNKNOWN_PRIVATE_CODE' }
  expect(oauth2OperationError(unknown)).toBe(unknown)
})
