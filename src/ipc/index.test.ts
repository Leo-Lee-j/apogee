import { describe, expect, it } from 'vitest'
import { toHistoryRequestPreview, type RequestSpec } from './index'

function requestSpec(): RequestSpec {
  return {
    method: 'POST',
    url: 'https://alice:password@example.test/v1/path-secret?token=query-secret&safe=ok#private',
    params: [
      { enabled: true, key: 'page', value: '1', kind: 'text', description: 'safe description' },
      { enabled: true, key: 'token', value: 'param-secret', kind: 'text' },
      { enabled: true, key: 'masked', value: 'kind-secret', kind: 'secret' },
    ],
    pathParams: [
      { enabled: true, key: 'secret', value: 'path-secret', kind: 'text' },
    ],
    headers: [
      { enabled: true, key: 'Authorization', value: 'Bearer header-secret', kind: 'text' },
      { enabled: true, key: 'Cookie', value: 'session=cookie-secret', kind: 'text' },
      { enabled: true, key: 'X-Trace', value: 'safe-header', kind: 'text' },
      { enabled: true, key: 'X-File', value: 'C:\\private\\upload.bin', kind: 'file' },
    ],
    body: { type: 'json', content: '{"token":"json-body-secret"}' },
    auth: { type: 'bearer', token: 'auth-secret' },
    // A runtime caller can add fields outside the TypeScript definition; the public converter
    // must still construct a whitelist rather than spread this object.
    settings: {
      timeoutMs: 1_000,
      proxyUrl: 'http://proxy-user:proxy-secret@proxy.test:8080',
    } as unknown as RequestSpec['settings'],
  }
}

describe('history request preview', () => {
  it('omits executable request surfaces and masks known credentials before IPC', () => {
    const preview = toHistoryRequestPreview(requestSpec())
    const encoded = JSON.stringify(preview)

    expect(preview.url).toBe('https://example.test/v1/[redacted]?token=%5Bredacted%5D&safe=ok')
    expect(preview.params?.map((row) => row.value)).toEqual(['1', '[redacted]', '[redacted]'])
    expect(preview.pathParams?.[0]?.value).toBe('[redacted]')
    expect(preview.headers?.map((row) => row.value)).toEqual([
      '[redacted]', '[redacted]', 'safe-header', '[file omitted]',
    ])
    expect(preview.settings?.timeoutMs).toBe(1_000)
    expect(preview).not.toHaveProperty('body')
    expect(preview).not.toHaveProperty('auth')

    for (const secret of [
      'password', 'query-secret', 'param-secret', 'kind-secret', 'path-secret',
      'header-secret', 'cookie-secret', 'auth-secret', 'json-body-secret', 'proxy-secret',
      'C:\\private', 'description',
    ]) {
      expect(encoded).not.toContain(secret)
    }
  })

  it('redacts sensitive query values and userinfo when a template URL cannot be parsed', () => {
    const preview = toHistoryRequestPreview({
      method: 'GET',
      url: '{{host}}/users?access_token=fallback-secret&safe=ok#private',
    })

    expect(preview.url).toBe('{{host}}/users?access_token=[redacted]&safe=ok')
    expect(JSON.stringify(preview)).not.toContain('fallback-secret')
  })

  it('uses neutral Secret sources for parsed and template query URLs and encoded paths', () => {
    const query = toHistoryRequestPreview({
      method: 'GET',
      url: 'https://api.test/search?q=query-secret&safe=ok',
      params: [{ enabled: true, key: 'q', value: 'query-secret', kind: 'secret' }],
    })
    const fallback = toHistoryRequestPreview({
      method: 'GET',
      url: '{{host}}/search?q=fallback-query-secret&safe=ok',
      params: [{ enabled: true, key: 'q', value: 'fallback-query-secret', kind: 'secret' }],
    })
    const path = toHistoryRequestPreview({
      method: 'GET',
      url: 'https://api.test/users/a%20b',
      pathParams: [{ enabled: true, key: 'id', value: 'a b', kind: 'secret' }],
    })
    const lowercaseEncodedPath = toHistoryRequestPreview({
      method: 'GET',
      url: 'https://api.test/users/%c3%a9',
      pathParams: [{ enabled: true, key: 'id', value: 'é', kind: 'secret' }],
    })

    expect(query.url).toBe('https://api.test/search?q=%5Bredacted%5D&safe=ok')
    expect(fallback.url).toBe('{{host}}/search?q=[redacted]&safe=ok')
    expect(path.url).toBe('https://api.test/users/[redacted]')
    expect(lowercaseEncodedPath.url).toBe('https://api.test/users/[redacted]')
    const encoded = JSON.stringify({ query, fallback, path, lowercaseEncodedPath })
    for (const secret of ['query-secret', 'fallback-query-secret', 'a b', 'a%20b', 'é', '%c3%a9']) {
      expect(encoded).not.toContain(secret)
    }
  })

  it('uses custom API-key names without forwarding auth to history IPC', () => {
    const queryPreview = toHistoryRequestPreview({
      method: 'GET',
      url: 'https://api.test/search?q=auth-query-secret&safe=ok',
      auth: { type: 'apiKey', key: 'q', value: 'auth-secret', location: 'query' },
    })
    const headerPreview = toHistoryRequestPreview({
      method: 'GET',
      url: 'https://api.test/search',
      headers: [{ enabled: true, key: 'X-My-Key', value: 'manual-header-secret', kind: 'text' }],
      auth: { type: 'apiKey', key: 'X-My-Key', value: 'auth-secret', location: 'header' },
    })

    expect(queryPreview.url).toBe('https://api.test/search?q=%5Bredacted%5D&safe=ok')
    expect(headerPreview.headers?.[0]?.value).toBe('[redacted]')
    const encoded = JSON.stringify({ queryPreview, headerPreview })
    for (const secret of ['auth-query-secret', 'manual-header-secret', 'auth-secret']) {
      expect(encoded).not.toContain(secret)
    }
    expect(queryPreview).not.toHaveProperty('auth')
    expect(headerPreview).not.toHaveProperty('auth')
  })
})
