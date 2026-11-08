import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Vue from 'vue'
import { createRenderer, defineComponent, h, nextTick, shallowRef, ssrContextKey } from 'vue'
import { parse, compileScript, compileTemplate } from 'vue/compiler-sfc'
import { transpileModule, ScriptTarget } from 'typescript'
import type { KeyValue } from '@/ipc'

const { openDialog, destroySortable } = vi.hoisted(() => ({ openDialog: vi.fn(), destroySortable: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }))
vi.mock('sortablejs', () => ({ default: { create: () => ({ destroy: destroySortable }) } }))
vi.mock('@/components/ui/icons', () => ({ IconCloseCircle: 'close', IconDrag: 'drag', IconFolderOpen: 'folder', IconRemoveCircle: 'remove' }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const stub = (name: string) => defineComponent({ inheritAttrs: false, setup: (_, { attrs, slots }) => () => h(name, attrs, slots.default?.()) })
  return {
    AppButton: stub('app-button'), AppCheckbox: stub('app-checkbox'), AppIcon: stub('app-icon'),
    AppInput: stub('app-input'), AppSelect: stub('app-select'),
  }
})
vi.mock('./VariableInput.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: (_, { attrs }) => () => h('variable-input', attrs) }) }
})

import ParameterEditor from './ParameterEditor.vue'
import source from './ParameterEditor.vue?raw'

// Node-mode Vitest emits SSR templates. Use the same SFC binding metadata to
// compile a client render function for the custom host, without a DOM dependency.
const { descriptor } = parse(source)
const { bindings } = compileScript(descriptor, { id: 'parameter-editor-test' })
const template = compileTemplate({
  source: descriptor.template!.content,
  filename: 'ParameterEditor.vue',
  id: 'parameter-editor-test',
  compilerOptions: { mode: 'function', bindingMetadata: bindings },
})
if (template.errors.length) throw new Error(String(template.errors[0]))
const renderCode = transpileModule(template.code, { compilerOptions: { target: ScriptTarget.ES2022 } }).outputText
Object.assign(ParameterEditor, { render: new Function('Vue', renderCode)(Vue) })

// 使用 Vue 真实更新/事件调度，但把输入控件和 DOM 宿主替换为轻量测试节点。
// 父组件在 nextTick 前不会刷新 props，能复现旧版分三次 emit 导致的新行拆分。
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
function mount(initial: KeyValue[] = [], extra: Record<string, unknown> = {}) {
  const root = node('root')
  const rows = shallowRef(initial)
  const owner = shallowRef('request-a')
  const appended = vi.fn((newRows: KeyValue[]) => { rows.value = [...rows.value, ...newRows] })
  const updated = vi.fn()
  const app = renderer.createApp(defineComponent({ setup: () => () => h(ParameterEditor, {
    key: owner.value, rows: rows.value, allowCreate: true, ...extra,
    onAppendRows: appended, onUpdateField: updated,
  }) }))
  app.provide(ssrContextKey, { modules: new Set() })
  app.mount(root)
  mounted.push(app)
  const draft = () => find(root, node => String(node.props.class ?? '').includes('parameter-row--draft'))[0]
  return { root, rows, owner, appended, updated, app, draft }
}
async function fillDraft(draft: Node, key = 'name', value = 'value', description = 'description') {
  const inputs = find(draft, node => node.type === 'variable-input')
  trigger(inputs[0], 'onUpdate:value', key)
  trigger(inputs[1], 'onUpdate:value', value)
  trigger(find(draft, node => node.type === 'app-input')[0], 'onUpdate:value', description)
  await nextTick()
}
function enter(input: Node, composing = false) {
  const preventDefault = vi.fn()
  trigger(input, 'onKeydown', { key: 'Enter', isComposing: composing, keyCode: composing ? 229 : 13, preventDefault })
  return preventDefault
}

beforeEach(() => { openDialog.mockReset(); openDialog.mockResolvedValue(null) })
afterEach(() => { for (const app of mounted.splice(0)) app.unmount() })

describe('ParameterEditor submission and ownership', () => {
  it('submits all draft fields with one atomic append event', async () => {
    const editor = mount()
    await fillDraft(editor.draft())
    enter(find(editor.draft(), node => node.type === 'variable-input')[1])
    expect(editor.appended).toHaveBeenCalledTimes(1)
    expect(editor.updated).not.toHaveBeenCalled()
    expect(editor.rows.value).toEqual([{ enabled: true, key: 'name', value: 'value', description: 'description', kind: 'text' }])
    await nextTick()
    expect(find(editor.draft(), node => node.type === 'variable-input')[0].props.value).toBe('')
  })
  it('does not commit while moving focus within the draft row', async () => {
    const editor = mount()
    await fillDraft(editor.draft())
    trigger(editor.draft(), 'onFocusout', { currentTarget: editor.draft(), relatedTarget: find(editor.draft(), node => node.type === 'app-input')[0] })
    expect(editor.appended).not.toHaveBeenCalled()
    trigger(editor.draft(), 'onFocusout', { currentTarget: editor.draft(), relatedTarget: null })
    expect(editor.appended).toHaveBeenCalledTimes(1)
  })
  it('does not treat IME confirmation as row submission', async () => {
    const editor = mount()
    await fillDraft(editor.draft())
    expect(enter(find(editor.draft(), node => node.type === 'variable-input')[0], true)).not.toHaveBeenCalled()
    expect(editor.appended).not.toHaveBeenCalled()
  })
  it('keeps draft file kind local until the whole row is committed', async () => {
    const editor = mount([], { showKind: true })
    trigger(find(editor.draft(), node => node.type === 'app-select')[0], 'onUpdate:value', 'file')
    await nextTick()
    expect(editor.rows.value).toEqual([])
    expect(editor.updated).not.toHaveBeenCalled()
    await fillDraft(editor.draft(), 'upload', 'C:\\file.txt', 'Attachment')
    enter(find(editor.draft(), node => node.type === 'variable-input')[0])
    expect(editor.rows.value[0].kind).toBe('file')
  })
  it('does not commit the draft when a native file dialog takes focus', async () => {
    let resolve: (value: string) => void = () => {}
    openDialog.mockImplementation(() => new Promise<string>(done => { resolve = done }))
    const editor = mount([], { showKind: true })
    trigger(find(editor.draft(), node => node.type === 'app-select')[0], 'onUpdate:value', 'file')
    await fillDraft(editor.draft(), 'upload', '', '')
    const pending = trigger(find(editor.draft(), node => node.props['aria-label'] === '选择新参数文件')[0], 'onClick')
    trigger(editor.draft(), 'onFocusout', { currentTarget: editor.draft(), relatedTarget: null })
    expect(editor.appended).not.toHaveBeenCalled()
    resolve('C:\\file.txt')
    await pending
    await nextTick()
    enter(find(editor.draft(), node => node.type === 'variable-input')[0])
    expect(editor.rows.value[0]).toMatchObject({ key: 'upload', value: 'C:\\file.txt', kind: 'file' })
  })
  it('rejects late focusout after the active request changed but before DOM unmount', async () => {
    let active = true
    const editor = mount([], { canEdit: () => active })
    await fillDraft(editor.draft())
    active = false
    trigger(editor.draft(), 'onFocusout', { currentTarget: editor.draft(), relatedTarget: null })
    expect(editor.appended).not.toHaveBeenCalled()
    expect(editor.rows.value).toEqual([])
  })
  it('resets the draft when its request owner changes', async () => {
    const editor = mount()
    await fillDraft(editor.draft())
    editor.owner.value = 'request-b'
    await nextTick()
    expect(find(editor.draft(), node => node.type === 'variable-input')[0].props.value).toBe('')
    enter(find(editor.draft(), node => node.type === 'variable-input')[0])
    expect(editor.appended).not.toHaveBeenCalled()
  })
  it('ignores a file dialog result after its request editor is unmounted', async () => {
    let resolve: (value: string) => void = () => {}
    openDialog.mockImplementation(() => new Promise<string>(done => { resolve = done }))
    const editor = mount([{ enabled: true, key: 'upload', value: '', kind: 'file' }], { showKind: true })
    const pending = trigger(find(editor.root, node => node.props['aria-label'] === '选择文件')[0], 'onClick')
    editor.owner.value = 'request-b'
    await nextTick()
    resolve('C:\\file.txt')
    await pending
    expect(editor.updated).not.toHaveBeenCalled()
  })
  it('shows actionable file-picker errors instead of rejecting an event handler', async () => {
    openDialog.mockRejectedValue(new Error('dialog unavailable'))
    const editor = mount([{ enabled: true, key: 'upload', value: '', kind: 'file' }], { showKind: true })
    await trigger(find(editor.root, node => node.props['aria-label'] === '选择文件')[0], 'onClick')
    await nextTick()
    const alerts = find(editor.root, node => node.props.role === 'alert')
    expect(alerts).toHaveLength(1)
    expect(find(alerts[0], node => node.text.includes('手动填写本地文件路径'))).not.toHaveLength(0)
    expect(editor.updated).not.toHaveBeenCalled()
  })
  it('appends a pasted Query String in one event', () => {
    const editor = mount()
    const preventDefault = vi.fn()
    trigger(find(editor.draft(), node => node.type === 'variable-input')[0], 'onPaste', { clipboardData: { getData: () => 'tag=a&tag=b&q=hello+world' }, preventDefault })
    expect(preventDefault).toHaveBeenCalledOnce()
    expect(editor.appended).toHaveBeenCalledOnce()
    expect(editor.rows.value.map(row => [row.key, row.value])).toEqual([['tag', 'a'], ['tag', 'b'], ['q', 'hello world']])
  })
  it('does not partially append malformed multi-row clipboard data', async () => {
    const editor = mount()
    trigger(find(editor.draft(), node => node.type === 'variable-input')[0], 'onPaste', { clipboardData: { getData: () => 'good: yes\nbad\tvalue\tdesc\tinvalid' }, preventDefault: vi.fn() })
    expect(editor.appended).not.toHaveBeenCalled()
    await nextTick()
    expect(find(editor.root, node => node.props.role === 'alert')).toHaveLength(1)
  })
  it('makes Path parameter names read-only and removes destructive controls', () => {
    const editor = mount([{ enabled: true, key: 'id', value: '123' }], { readOnlyKey: true, allowCreate: false, allowRemove: false })
    const input = find(editor.root, node => node.type === 'variable-input')[0]
    expect(input.props.readonly).toBe(true)
    expect(input.props.variables).toBe(false)
    expect(find(editor.root, node => node.props.title === '删除参数')).toHaveLength(0)
    expect(editor.draft()).toBeUndefined()
  })
})
