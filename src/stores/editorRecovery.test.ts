import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { RecoverySnapshot } from '@/ipc'
const mocks = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn(), save: vi.fn(), readRequest: vi.fn(), saveRequest: vi.fn(), choice: vi.fn() }))
vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { workspaceReadRequest: mocks.readRequest, workspaceSaveRequest: mocks.saveRequest, workspaceReadRecovery: mocks.read, workspaceSaveRecovery: mocks.write, workspaceSaveContainer: mocks.save },
  normalizeError: (error: unknown) => ({ kind: 'io', message: error instanceof Error ? error.message : String(error), hints: [] }),
}))
vi.mock('@/features/feedback', () => ({ confirmUnsaved: mocks.choice, toast: () => ({ error: vi.fn() }), errorText: (text: string) => text }))
import { useTabsStore } from './tabs'
import { useEditorRecoveryStore } from './editorRecovery'

let saved: RecoverySnapshot
beforeEach(() => {
  setActivePinia(createPinia()); vi.useFakeTimers(); vi.resetAllMocks()
  saved = { requests: [], containers: [] }
  mocks.read.mockImplementation(async () => structuredClone(saved))
  mocks.write.mockImplementation(async value => { saved = structuredClone(value) })
  mocks.save.mockImplementation(async value => value)
})
afterEach(() => { useEditorRecoveryStore().stop(); useTabsStore().resetWorkspace(); vi.useRealTimers() })
describe('editor recovery and leave protection', () => {
  it('retains clean saved Body drafts through autosave, close, preview reopen and workspace recovery', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start()
    const tab = tabs.open({ nodeId: 'saved', body: { type: 'json', content: 'fixture' } })
    tabs.switchBodyType(tab.id, 'none'); await tabs.save(tab.id)
    expect(tabs.active?.dirty).toBe(false)
    await tabs.close(tab.id)
    expect(saved.requests[0]).toMatchObject({ dirty: false, bodyDrafts: { json: { content: 'fixture' } } })
    tabs.open({ preview: true })
    const reopened = tabs.open({ nodeId: 'saved', preview: true })
    expect(reopened.bodyDrafts.json).toMatchObject({ content: 'fixture' })
    expect(reopened.preview).toBe(false)
    await store.flush(); tabs.resetWorkspace()
    await store.start()
    expect(tabs.active).toBeNull()
    tabs.open({ ...saved.requests[0]!.spec, nodeId: 'saved', title: 'Latest title', auth: { type: 'bearer', token: 'rotated' } })
    expect(tabs.active?.dirty).toBe(false)
    expect(tabs.active?.persistedAuth).toEqual({ type: 'bearer', token: 'rotated' })
    tabs.switchBodyType(tabs.active!.id, 'json')
    expect(tabs.active?.body).toEqual({ type: 'json', content: 'fixture' })
  })
  it('opens a deleted source detached by Rust as an editable draft with its inactive Body types', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    saved.requests.push({
      id: 'recovered', title: 'Deleted request', dirty: true, nodeId: null, expectedAuth: null, debugCaseEditor: false,
      spec: { method: 'POST', url: 'https://fixture.test', params: [], pathParams: [], headers: [], body: { type: 'none' }, auth: { type: 'none' } },
      bodyDrafts: { json: { type: 'json', content: 'preserved fixture' } },
    })
    await store.start()
    expect(tabs.active).toMatchObject({ title: 'Deleted request', dirty: true, nodeId: undefined })
    tabs.switchBodyType(tabs.active!.id, 'json')
    expect(tabs.active?.body).toEqual({ type: 'json', content: 'preserved fixture' })
    expect(mocks.readRequest).not.toHaveBeenCalled()
  })
  it('cancels closing without losing a draft, then saves and restores the closed draft', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start()
    const tab = tabs.open({ title: 'draft', url: 'https://draft.test', dirty: true })
    mocks.choice.mockResolvedValueOnce('cancel')
    await tabs.close(tab.id)
    expect(tabs.tabs).toHaveLength(1)
    mocks.choice.mockResolvedValueOnce('save')
    await tabs.close(tab.id)
    expect(tabs.tabs).toHaveLength(0)
    expect(saved.requests).toHaveLength(1)
    expect(saved.requests[0]?.spec.url).toBe('https://draft.test')
    await store.start()
    expect(tabs.active).toMatchObject({ title: 'draft', dirty: true, url: 'https://draft.test', recoveryId: tab.recoveryId })
    expect(tabs.active?.id).not.toBe(tab.id)
  })
  it('keeps the tab on failed storage and discards it only on an explicit choice', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start()
    const tab = tabs.open({ url: 'https://draft.test', dirty: true })
    mocks.choice.mockResolvedValue('save'); mocks.write.mockRejectedValueOnce(new Error('disk full'))
    await tabs.close(tab.id)
    expect(tabs.tabs).toHaveLength(1)
    expect(store.lastError?.message).toBe('disk full')
    mocks.choice.mockResolvedValue('discard')
    await tabs.close(tab.id)
    expect(saved.requests).toHaveLength(0)
  })
  it('coalesces writes and includes edits arriving during a pending write', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start()
    const tab = tabs.open({ dirty: true })
    let finish!: () => void
    mocks.write.mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const first = store.flush()
    tabs.patch(tab.id, { url: 'https://latest.test', dirty: true })
    const second = store.flush()
    expect(mocks.write).toHaveBeenCalledTimes(1)
    finish()
    expect(await first).toBe(true); expect(await second).toBe(true)
    expect(mocks.write).toHaveBeenCalledTimes(2)
    expect(saved.requests[0]?.spec.url).toBe('https://latest.test')
    expect(saved.requests[0]).not.toHaveProperty('response')
  })
  it('does not write the previous workspace snapshot after a session reset', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start(); tabs.open({ dirty: true })
    let finish!: () => void
    mocks.write.mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const writing = store.flush()
    tabs.patch(tabs.activeId!, { url: 'late' })
    store.stop(); tabs.resetWorkspace(); finish()
    expect(await writing).toBe(false)
    await vi.advanceTimersByTimeAsync(400)
    expect(mocks.write).toHaveBeenCalledTimes(1)
  })
  it('retains container edits and a newer auth baseline through concurrent saves', async () => {
    const store = useEditorRecoveryStore()
    await store.start()
    const original = { id: 'container', name: 'C', auth: { type: 'inherit' as const }, settings: {} }
    store.put(original); store.edit({ ...original, settings: { timeoutMs: 1000 } })
    let finish!: () => void
    mocks.save.mockImplementationOnce(value => new Promise(resolve => { finish = () => resolve(value) }))
    const writing = store.saveContainer('container')
    const auth = { type: 'bearer' as const, token: 'new' }
    store.edit({ ...store.containers.container!.value, auth }); store.baseline('container', auth)
    finish(); expect(await writing).toBe(true)
    expect(mocks.save.mock.calls[1]?.[1]).toEqual(auth)
    expect(store.containers.container).toMatchObject({ dirty: false, value: { auth } })
  })
  it('cancels leaving without altering draft or container dirty state', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start(); tabs.open({ dirty: true })
    const value = { id: 'c', name: 'C', auth: { type: 'inherit' as const }, settings: {} }
    store.put(value); store.edit(value)
    mocks.choice.mockResolvedValueOnce('cancel')
    expect(await store.prepareLeave()).toBe(false)
    expect(tabs.active?.dirty).toBe(true)
    expect(store.containers.c?.dirty).toBe(true)
    expect(mocks.write).not.toHaveBeenCalled()
  })
  it('keeps discarded edits available when the recovery file cannot be updated', async () => {
    const store = useEditorRecoveryStore(), tabs = useTabsStore()
    await store.start(); tabs.open({ dirty: true, url: 'keep-me' })
    const value = { id: 'c', name: 'C', auth: { type: 'inherit' as const }, settings: {} }
    store.put(value); store.edit(value)
    mocks.choice.mockResolvedValue('discard'); mocks.write.mockRejectedValueOnce(new Error('disk full'))
    expect(await store.prepareLeave()).toBe(false)
    expect(tabs.active).toMatchObject({ dirty: true, url: 'keep-me' })
    expect(store.containers.c?.dirty).toBe(true)
  })
  it('does not overwrite dirty container edits when reloading a page', async () => {
    const store = useEditorRecoveryStore(); await store.start()
    const value = { id: 'c', name: 'C', auth: { type: 'inherit' as const }, settings: {} }
    store.put(value); store.edit({ ...value, settings: { timeoutMs: 1234 } })
    store.put({ ...value, settings: { timeoutMs: 9999 } })
    expect(store.containers.c?.value.settings.timeoutMs).toBe(1234)
  })
})
