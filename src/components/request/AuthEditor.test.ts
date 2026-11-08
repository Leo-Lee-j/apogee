import { afterEach, describe, expect, it, vi } from 'vitest'
import * as Vue from 'vue'
import { createRenderer, defineComponent, h, nextTick, shallowRef, ssrContextKey } from 'vue'
import { parse, compileScript, compileTemplate } from 'vue/compiler-sfc'
import { transpileModule, ScriptTarget } from 'typescript'
import { ipc, type AuthConfig, type OAuth2TokenResponse } from '@/ipc'
import { selectAuth } from '@/features/requestAuth'
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const stub = (name: string) => defineComponent({ inheritAttrs: false, setup: (_, { attrs, slots }) => () => h(name, attrs, slots.default?.()) })
  return { AppSwitch: stub('app-switch'), AppInput: stub('app-input'), AppSelect: stub('app-select'), AppButton: stub('app-button'), AppAlert: stub('app-alert') }
})
import AuthEditor from './AuthEditor.vue'
import source from './AuthEditor.vue?raw'
const { descriptor } = parse(source)
const { bindings } = compileScript(descriptor, { id: 'auth-editor-test' })
const template = compileTemplate({ source: descriptor.template!.content, filename: 'AuthEditor.vue', id: 'auth-editor-test',
  compilerOptions: { mode: 'function', bindingMetadata: bindings } })
if (template.errors.length) throw new Error(String(template.errors[0]))
const code = transpileModule(template.code, { compilerOptions: { target: ScriptTarget.ES2022 } }).outputText
Object.assign(AuthEditor, { render: new Function('Vue', code)(Vue) })
interface Node {
  type: string
  props: Record<string, unknown>
  children: Node[]
  parent: Node | null
  text: string
  contains: (node: Node | null) => boolean
}
function node(type: string, text = ''): Node {
  const result: Node = { type, text, props: {}, children: [], parent: null, contains: candidate => candidate === result || result.children.some(child => child.contains(candidate)) }
  return result
}
const renderer = createRenderer<Node, Node>({
  createElement: type => node(type),
  createText: text => node('#text', text),
  createComment: text => node('#comment', text),
  setText: (node, text) => { node.text = text },
  setElementText: (node, text) => { node.text = text; node.children = [] },
  parentNode: node => node.parent,
  nextSibling: node => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
  patchProp: (node, key, _, value) => { node.props[key] = value },
  insert: (node, parent, anchor) => {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1)
    const position = anchor ? parent.children.indexOf(anchor) : -1
    parent.children.splice(position < 0 ? parent.children.length : position, 0, node)
    node.parent = parent
  },
  remove: node => {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1)
    node.parent = null
  },
})
function find(root: Node, predicate: (node: Node) => boolean): Node[] {
  return [...(predicate(root) ? [root] : []), ...root.children.flatMap(child => find(child, predicate))]
}
function trigger(node: Node, name: string, ...args: unknown[]) {
  const handler = node.props[name]
  expect(handler).toBeTypeOf('function')
  return (handler as (...args: unknown[]) => unknown)(...args)
}

const mounted: { unmount: () => void }[] = []
function mount(type: AuthConfig['type'] = 'none', extra: Record<string, unknown> = {}) {
  const root = node('root')
  const value = shallowRef(selectAuth(type, { type: 'inherit' }))
  const updated = vi.fn((next: AuthConfig) => { value.value = next })
  const app = renderer.createApp(defineComponent({ setup: () => () => h(AuthEditor, { value: value.value, ...extra, 'onUpdate:value': updated }) }))
  app.provide(ssrContextKey, { modules: new Set() }); app.mount(root); mounted.push(app)
  return { root, value, updated, inputs: () => find(root, n => n.type === 'app-input'), selects: () => find(root, n => n.type === 'app-select') }
}
afterEach(() => { for (const app of mounted.splice(0)) app.unmount(); vi.restoreAllMocks() })
describe('AuthEditor', () => {
  it('selects OAuth1 and persists fields and signature algorithm through actual listeners', async () => {
    const editor = mount()
    trigger(editor.selects()[0], 'onUpdate:value', 'oauth1'); await nextTick()
    expect(editor.inputs()).toHaveLength(9)
    trigger(editor.inputs()[0], 'onUpdate:value', '{{consumer}}'); await nextTick()
    trigger(editor.inputs()[1], 'onUpdate:value', 'private-secret'); await nextTick()
    trigger(editor.selects()[1], 'onUpdate:value', 'HMAC-SHA256'); await nextTick()
    expect(editor.value.value).toMatchObject({ type: 'oauth1', consumerKey: '{{consumer}}', consumerSecret: 'private-secret', signatureMethod: 'HMAC-SHA256' })
    expect(editor.inputs()[1].props.type).toBe('password')
    expect(editor.inputs()[2].props.type).toBe('password')
  })
  it('edits OAuth2 prefix and JWT JSON fields without exposing secret inputs', async () => {
    const editor = mount('oauth2')
    trigger(editor.inputs()[0], 'onUpdate:value', 'access-token'); await nextTick()
    trigger(editor.inputs()[1], 'onUpdate:value', ''); await nextTick()
    expect(editor.value.value).toEqual({ type: 'oauth2', accessToken: 'access-token', headerPrefix: '' })
    trigger(editor.selects()[0], 'onUpdate:value', 'jwt'); await nextTick()
    trigger(editor.inputs()[1], 'onUpdate:value', '{"sub":"{{user}}"}'); await nextTick()
    trigger(editor.selects()[1], 'onUpdate:value', 'HS512'); await nextTick()
    expect(editor.value.value).toMatchObject({ type: 'jwt', payload: '{"sub":"{{user}}"}', algorithm: 'HS512' })
    expect(editor.inputs().slice(0, 3).every(n => n.props.type === 'password')).toBe(true)
    expect(editor.value.value).not.toHaveProperty('accessToken')
  })
  it('ignores no-op updates and late field events after switching schemes', async () => {
    const editor = mount('bearer'); const late = editor.inputs()[0].props['onUpdate:value'] as (value: string) => void
    trigger(editor.inputs()[0], 'onUpdate:value', ''); expect(editor.updated).not.toHaveBeenCalled()
    trigger(editor.selects()[0], 'onUpdate:value', 'oauth2'); await nextTick()
    late('old-secret'); expect(editor.value.value).not.toHaveProperty('token')
    expect(editor.value.value).toMatchObject({ type: 'oauth2', accessToken: '' })
  })
  it('rejects stale owner events before parent unmount and handles collection inheritance options', async () => {
    let owner = 'a'; const editor = mount('oauth1', { canEdit: () => owner === 'a', allowInherit: false })
    const options = editor.selects()[0].props.options as { value: string }[]
    expect(options.some(o => o.value === 'inherit')).toBe(false)
    owner = 'b'
    trigger(editor.inputs()[1], 'onUpdate:value', 'wrong-owner-secret')
    trigger(editor.selects()[0], 'onUpdate:value', 'jwt')
    expect(editor.updated).not.toHaveBeenCalled()
  })
})

function text(root: Node): string { return root.text + root.children.map(text).join('') }
function button(root: Node, label: string): Node { const result = find(root, n => n.type === 'app-button' && text(n) === label)[0]; expect(result).toBeTruthy(); return result }
async function configure(editor: ReturnType<typeof mount>) {
  trigger(button(editor.root, '配置 Token 获取'), 'onClick'); await nextTick()
  editor.value.value = { ...editor.value.value, tokenRequest: { tokenUrl: 'https://auth.test/token', clientId: 'client', clientSecret: 'private-secret', scope: '', refreshToken: 'old-refresh', clientAuth: 'basic' } } as AuthConfig
  await nextTick()
}
function deferred() {
  let resolve!: (response: OAuth2TokenResponse | null) => void
  const promise = new Promise<OAuth2TokenResponse | null>(r => { resolve = r })
  return { promise, resolve }
}
describe('AuthEditor token operations', () => {
  it('fetches a token and rotates refresh credentials only in the initiating auth config', async () => {
    const fetchToken = vi.fn().mockResolvedValue({ accessToken: 'new-access', refreshToken: 'new-refresh', expiresIn: 3600 })
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    await trigger(button(editor.root, '获取 Token'), 'onClick'); await nextTick()
    expect(fetchToken).toHaveBeenCalledWith(expect.objectContaining({ clientSecret: 'private-secret' }), 'clientCredentials', expect.any(String), expect.any(AbortSignal), expect.objectContaining({ type: 'oauth2' }))
    expect(editor.value.value).toMatchObject({ accessToken: 'new-access', tokenRequest: { refreshToken: 'new-refresh' } })
    expect(text(editor.root)).toContain('有效期 3600 秒')
    expect(editor.inputs().find(n => n.props.value === 'new-refresh')?.props.type).toBe('password')
  })
  it('refreshes explicitly and retains an old refresh token when the server omits rotation', async () => {
    const fetchToken = vi.fn().mockResolvedValue({ accessToken: 'refreshed' })
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    await trigger(button(editor.root, '刷新 Token'), 'onClick'); await nextTick()
    expect(fetchToken.mock.calls[0][1]).toBe('refreshToken')
    expect(editor.value.value).toMatchObject({ accessToken: 'refreshed', tokenRequest: { refreshToken: 'old-refresh' } })
  })
  it('cancels an in-flight operation on configuration edits and drops its late token', async () => {
    const cancel = vi.spyOn(ipc, 'cancelOAuth2Token').mockResolvedValue(true)
    const pending = deferred(); const fetchToken = vi.fn().mockReturnValue(pending.promise)
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    const task = trigger(button(editor.root, '获取 Token'), 'onClick'); await nextTick()
    expect(button(editor.root, '获取 Token').props.disabled).toBe(true)
    editor.value.value = { ...editor.value.value, headerPrefix: 'Token' } as AuthConfig; await nextTick()
    expect(cancel).toHaveBeenCalledWith(fetchToken.mock.calls[0][2])
    expect(fetchToken.mock.calls[0][3].aborted).toBe(true)
    pending.resolve({ accessToken: 'late-private' }); await task; await nextTick()
    expect(editor.value.value).not.toMatchObject({ accessToken: 'late-private' })
  })
  it('cancels explicitly and never overwrites a subsequent operation with an old result', async () => {
    vi.spyOn(ipc, 'cancelOAuth2Token').mockResolvedValue(true)
    const old = deferred(); const next = deferred(); const fetchToken = vi.fn().mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise)
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    const first = trigger(button(editor.root, '获取 Token'), 'onClick'); await nextTick()
    trigger(button(editor.root, '取消'), 'onClick'); await nextTick()
    const second = trigger(button(editor.root, '获取 Token'), 'onClick'); await nextTick()
    old.resolve({ accessToken: 'old-private' }); await first; await nextTick()
    expect(button(editor.root, '获取 Token').props.disabled).toBe(true)
    next.resolve({ accessToken: 'current-token' }); await second; await nextTick()
    expect(editor.value.value).toMatchObject({ accessToken: 'current-token' })
  })
  it('ignores a late result after ownership changes and renders only structured errors', async () => {
    vi.spyOn(ipc, 'cancelOAuth2Token').mockResolvedValue(true)
    let owner = true; const pending = deferred()
    const editor = mount('oauth2', { canEdit: () => owner, fetchToken: () => pending.promise }); await configure(editor)
    const task = trigger(button(editor.root, '获取 Token'), 'onClick'); owner = false
    pending.resolve({ accessToken: 'wrong-owner-secret' }); await task; await nextTick()
    expect(editor.value.value).not.toMatchObject({ accessToken: 'wrong-owner-secret' })
    const failed = mount('oauth2', { fetchToken: vi.fn().mockRejectedValue({ kind: 'http', message: 'invalid_client', hints: [{ label: '检查客户端配置' }] }) })
    await configure(failed); await trigger(button(failed.root, '获取 Token'), 'onClick'); await nextTick()
    const alert = find(failed.root, n => n.type === 'app-alert')[0]
    expect(alert.props.title).toBe('invalid_client'); expect(text(alert)).toContain('检查客户端配置')
  })
})

describe('AuthEditor PKCE operations', () => {
  it('selects PKCE, edits endpoints and sends the authorization-code grant', async () => {
    const fetchToken = vi.fn().mockResolvedValue({ accessToken: 'pkce-access', refreshToken: 'pkce-refresh' })
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    trigger(editor.selects()[1], 'onUpdate:value', 'authorizationCode'); await nextTick()
    expect(editor.value.value).toMatchObject({ tokenRequest: { grantType: 'authorizationCode', redirectUri: 'http://127.0.0.1:53682/oauth/callback' } })
    const input = editor.inputs().find(n => n.props.placeholder === 'https://auth.example.com/oauth/authorize')!
    trigger(input, 'onUpdate:value', 'https://auth.test/authorize'); await nextTick()
    await trigger(button(editor.root, '浏览器授权获取 Token'), 'onClick'); await nextTick()
    expect(fetchToken).toHaveBeenCalledWith(expect.objectContaining({ grantType: 'authorizationCode', authorizationUrl: 'https://auth.test/authorize' }), 'authorizationCode', expect.any(String), expect.any(AbortSignal), expect.objectContaining({ type: 'oauth2' }))
    expect(editor.value.value).toMatchObject({ accessToken: 'pkce-access', tokenRequest: { refreshToken: 'pkce-refresh' } })
  })
  it('cancels browser waiting when the grant changes and ignores its late result', async () => {
    const cancel = vi.spyOn(ipc, 'cancelOAuth2Token').mockResolvedValue(true)
    const pending = deferred(); const fetchToken = vi.fn().mockReturnValue(pending.promise)
    const editor = mount('oauth2', { fetchToken }); await configure(editor)
    trigger(editor.selects()[1], 'onUpdate:value', 'authorizationCode'); await nextTick()
    const task = trigger(button(editor.root, '浏览器授权获取 Token'), 'onClick'); await nextTick()
    expect(text(editor.root)).toContain('等待系统浏览器授权')
    trigger(editor.selects()[1], 'onUpdate:value', 'clientCredentials'); await nextTick()
    expect(cancel).toHaveBeenCalledWith(fetchToken.mock.calls[0][2]); expect(fetchToken.mock.calls[0][3].aborted).toBe(true)
    pending.resolve({ accessToken: 'late-pkce-token' }); await task; await nextTick()
    expect(editor.value.value).not.toMatchObject({ accessToken: 'late-pkce-token' })
    expect(button(editor.root, '获取 Token')).toBeTruthy()
  })
})

it('ignores no-op and stale grant selector events', async () => {
  const editor = mount('oauth2'); await configure(editor); editor.updated.mockClear()
  trigger(editor.selects()[1], 'onUpdate:value', 'clientCredentials')
  expect(editor.updated).not.toHaveBeenCalled()
  const late = editor.selects()[1].props['onUpdate:value'] as (value: string) => void
  editor.value.value = { ...editor.value.value, headerPrefix: 'Token' } as AuthConfig; await nextTick()
  late('authorizationCode')
  expect(editor.updated).not.toHaveBeenCalled()
})

it('persists acquisition/expiry and clears stale expiry after manual token replacement', async () => {
  const editor = mount('oauth2', { fetchToken: vi.fn().mockResolvedValue({ accessToken: 'new', expiresIn: 60 }) })
  await configure(editor)
  await trigger(find(editor.root, n => n.type === 'app-button' && n.children.some(c => c.text === '获取 Token'))[0], 'onClick')
  await nextTick()
  const auth = editor.value.value
  if (auth.type !== 'oauth2') throw new Error('fixture')
  expect(auth.obtainedAt).toEqual(expect.any(Number)); expect(auth.expiresAt! - auth.obtainedAt!).toBe(60000)
  trigger(editor.inputs()[0], 'onUpdate:value', 'pasted-token'); await nextTick()
  expect(editor.value.value).not.toHaveProperty('expiresAt'); expect(editor.value.value).not.toHaveProperty('obtainedAt')
})
it('auto refresh is request-only opt-in and rejects a stale switch event', async () => {
  const editor = mount('oauth2', { allowAutoRefresh: true })
  const control = find(editor.root, n => n.type === 'app-switch')[0]
  const stale = control.props['onUpdate:value'] as (value: boolean) => void
  trigger(control, 'onUpdate:value', true); await nextTick()
  expect(editor.value.value).toMatchObject({ autoRefresh: true })
  editor.updated.mockClear(); stale(false); expect(editor.updated).not.toHaveBeenCalled()
  const container = mount('oauth2'); expect(find(container.root, n => n.type === 'app-switch')).toHaveLength(0)
})
