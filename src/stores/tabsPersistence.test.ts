import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const { workspaceSaveRequest, showError } = vi.hoisted(() => ({
  workspaceSaveRequest: vi.fn(),
  showError: vi.fn(),
}))

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { workspaceSaveRequest },
  normalizeError: (error: unknown) => ({
    kind: 'io',
    message: error instanceof Error ? error.message : String(error),
    hints: [{ label: '检查目录权限后重试' }],
  }),
}))
vi.mock('@/features/feedback', () => ({
  toast: () => ({ error: showError }),
  errorText: (message: string) => message,
}))

import { useTabsStore } from './tabs'

function deferred() {
  let resolve!: () => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}

describe('request tab persistence', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    vi.resetAllMocks()
    workspaceSaveRequest.mockResolvedValue(undefined)
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    useTabsStore().resetWorkspace()
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('pins a preview as soon as it is edited so the next preview cannot replace it', () => {
    const tabs = useTabsStore()
    const edited = tabs.open({ nodeId: 'a', preview: true })
    tabs.patch(edited.id, { url: 'https://edited.test', dirty: true })
    const next = tabs.open({ nodeId: 'b', preview: true })

    expect(next.id).not.toBe(edited.id)
    expect(tabs.tabs.find((tab) => tab.id === edited.id)).toMatchObject({
      url: 'https://edited.test', preview: false, dirty: true,
    })
  })

  it('does not mark a container configuration tab dirty when its title refreshes', () => {
    const tabs = useTabsStore()
    const tab = tabs.open({ kind: 'container-config', title: '配置' })
    tabs.patch(tab.id, { title: '配置 · 集合' })
    expect(tabs.active?.dirty).toBe(false)
  })

  it('syncs path rows while preserving values and leaving a preview unpinned', () => {
    const tabs = useTabsStore()
    const existing = { enabled: false, key: 'id', value: '42', kind: 'secret' as const, description: 'keep' }
    const tab = tabs.open({ preview: true, pathParams: [existing] })
    tabs.syncPathParams(tab.id, 'https://user:password@host/:id/{next}?q=:ignored')
    expect(tabs.active?.pathParams[0]).toEqual(existing)
    expect(tabs.active?.pathParams.map((row) => row.key)).toEqual(['id', 'next'])
    expect(tabs.active?.preview).toBe(true)
    expect(tabs.active?.dirty).toBe(false)
  })

  it('serializes automatic and explicit saves and only marks the newest edit clean', async () => {
    const first = deferred()
    const second = deferred()
    workspaceSaveRequest.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request' })
    tabs.patch(tab.id, { url: 'https://first.test', dirty: true })
    await vi.advanceTimersByTimeAsync(300)
    tabs.patch(tab.id, { url: 'https://latest.test', dirty: true })
    const saving = tabs.save(tab.id)

    expect(workspaceSaveRequest).toHaveBeenCalledTimes(1)
    first.resolve()
    await vi.advanceTimersByTimeAsync(0)
    expect(workspaceSaveRequest).toHaveBeenCalledTimes(2)
    expect(workspaceSaveRequest.mock.calls[1]![2].url).toBe('https://latest.test')
    expect(tabs.active?.dirty).toBe(true)

    second.resolve()
    await expect(saving).resolves.toEqual({ ok: true, error: null })
    expect(tabs.active?.dirty).toBe(false)
    await vi.advanceTimersByTimeAsync(300)
    expect(workspaceSaveRequest).toHaveBeenCalledTimes(2)
  })

  it('shows an automatic save failure while keeping the unsaved edit', async () => {
    workspaceSaveRequest.mockRejectedValueOnce(new Error('disk full'))
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request' })
    tabs.patch(tab.id, { url: 'https://edited.test', dirty: true })
    await vi.advanceTimersByTimeAsync(300)

    expect(tabs.active?.dirty).toBe(true)
    expect(showError).toHaveBeenCalledWith('disk full')
  })

  it('retains a saved request tab if saving on close fails', async () => {
    workspaceSaveRequest.mockRejectedValueOnce(new Error('read only'))
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', dirty: true })

    await tabs.close(tab.id)

    expect(tabs.tabs.map((item) => item.id)).toContain(tab.id)
    expect(tabs.active?.dirty).toBe(true)
    expect(showError).toHaveBeenCalledWith('read only')
    await tabs.close(tab.id)
    expect(tabs.tabs).toHaveLength(0)
  })

  it('waits for a closing tab save before flushing a workspace', async () => {
    const pending = deferred()
    workspaceSaveRequest.mockReturnValueOnce(pending.promise)
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', dirty: true })
    const closing = tabs.close(tab.id)
    let flushed = false
    const flushing = tabs.flushAll().then((ok) => { flushed = true; return ok })
    await vi.advanceTimersByTimeAsync(0)
    expect(flushed).toBe(false)
    expect(tabs.tabs).toHaveLength(1)

    pending.resolve()
    await closing
    await expect(flushing).resolves.toBe(true)
    expect(tabs.tabs).toHaveLength(0)
  })

  it('does not resume a pending save after the workspace has been reset', async () => {
    const pending = deferred()
    workspaceSaveRequest.mockReturnValueOnce(pending.promise)
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', dirty: true })
    const saving = tabs.save(tab.id)
    tabs.patch(tab.id, { url: 'https://old-workspace.test' })
    tabs.resetWorkspace()
    const reopened = tabs.open({ nodeId: 'request', url: 'https://new-workspace.test', dirty: true })
    pending.resolve()

    await expect(saving).resolves.toEqual({ ok: false, error: null })
    expect(workspaceSaveRequest).toHaveBeenCalledTimes(1)
    expect(tabs.active?.id).toBe(reopened.id)
    expect(tabs.active?.dirty).toBe(true)
  })
})
