import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import * as Vue from 'vue'
import { createRenderer, defineComponent, h, nextTick, ssrContextKey } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import { parse, compileScript, compileTemplate } from 'vue/compiler-sfc'
import { transpileModule, ScriptTarget } from 'typescript'
import { ipc } from '@/ipc'
import { useUpdateStore } from '@/stores/update'
vi.mock('@/ipc', async importOriginal => ({ ...await importOriginal<typeof import('@/ipc')>(), inTauri: true }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const stub = (name: string) => defineComponent({ inheritAttrs: false, setup: (_, { attrs, slots }) => () => h(name, attrs, [slots.trigger?.(), slots.default?.()]) })
  return { AppAlert: stub('app-alert'), AppButton: stub('app-button'), AppSwitch: stub('app-switch'), AppPopconfirm: stub('app-popconfirm') }
})
import UpdatePanel from './UpdatePanel.vue'
import source from './UpdatePanel.vue?raw'
const { descriptor } = parse(source)
const { bindings } = compileScript(descriptor, { id: 'update-panel-test' })
const template = compileTemplate({ source: descriptor.template!.content, filename: 'UpdatePanel.vue', id: 'update-panel-test',
  compilerOptions: { mode: 'function', bindingMetadata: bindings } })
if (template.errors.length) throw new Error(String(template.errors[0]))
const code = transpileModule(template.code, { compilerOptions: { target: ScriptTarget.ES2022 } }).outputText
Object.assign(UpdatePanel, { render: new Function('Vue', code)(Vue) })
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
let unmount: (() => void) | undefined
function mount() {
  const root = node('root')
  const app = renderer.createApp(defineComponent({ setup: () => () => h(UpdatePanel) }))
  app.provide(ssrContextKey, {})
  app.mount(root)
  unmount = () => app.unmount()
  return root
}
function text(node: Node): string { return node.text + node.children.map(text).join('') }
beforeEach(() => {
  setActivePinia(createPinia())
  vi.spyOn(ipc, 'updateCurrentVersion').mockResolvedValue('0.1.0')
})
afterEach(() => { unmount?.(); unmount = undefined; vi.restoreAllMocks() })

describe('update panel', () => {
  it('shows the manual check action without starting network activity on mount', async () => {
    const check = vi.spyOn(ipc, 'updateCheck')
    const root = mount(); await nextTick()
    expect(text(root)).toContain('检查更新')
    expect(text(root)).toContain('Windows x64')
    expect(text(root)).toContain('未签名安装包可能触发 Windows 安全提示')
    expect(text(root)).not.toContain('稳定版发布要求 Windows 签名')
    expect(check).not.toHaveBeenCalled()
    expect(find(root, n => n.type === 'app-switch')[0]!.props.disabled).toBe(false)
  })
  it('renders release notes as text and shows bounded download progress with cancellation', async () => {
    const store = useUpdateStore()
    store.latest = { tag: 'v0.2.0', version: '0.2.0', notes: '<script>unsafe()</script>', publishedAt: null,
      prerelease: true, filename: 'Apogee_0.2.0_x64-setup.exe', size: 100, sha256: 'a'.repeat(64) }
    store.phase = 'downloading'; store.updateAvailable = true; store.receivedBytes = 50; store.totalBytes = 100
    const root = mount(); await nextTick()
    expect(text(root)).toContain('<script>unsafe()</script>')
    expect(find(root, n => n.type === 'script')).toHaveLength(0)
    expect(find(root, n => n.type === 'progress')[0]!.props.value).toBe(50)
    expect(text(root)).toContain('取消下载')
    expect(find(root, n => n.type === 'app-switch')[0]!.props.disabled).toBe(true)
  })
  it('requires the confirmation callback before opening a verified installer', async () => {
    const store = useUpdateStore()
    store.latest = { tag: 'v0.2.0', version: '0.2.0', notes: '', publishedAt: null,
      prerelease: false, filename: 'Apogee_0.2.0_x64-setup.exe', size: 100, sha256: 'a'.repeat(64) }
    store.phase = 'ready'; store.downloaded = { tag: 'v0.2.0', filename: store.latest.filename, path: 'C:/cache/setup.exe' }
    const open = vi.spyOn(store, 'openDownloaded').mockResolvedValue()
    const root = mount(); await nextTick()
    const confirm = find(root, n => n.type === 'app-popconfirm')[0]!
    expect(text(confirm)).toContain('请先保存未保存的请求')
    expect(open).not.toHaveBeenCalled()
    await (confirm.props.onPositiveClick as () => Promise<void>)()
    expect(open).toHaveBeenCalledWith(false)
  })
})
