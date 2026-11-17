import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { ExecHandle, ResponseSummary } from '@/ipc'

const { sendRequest, cancelRequest, historyRecord, lifecycle, events } = vi.hoisted(() => ({
  sendRequest: vi.fn(),
  cancelRequest: vi.fn(),
  historyRecord: vi.fn(),
  lifecycle: { unmount: () => {} },
  events: { handlers: {} as Parameters<typeof import('@/ipc')['onRequestEvents']>[0] },
}))

vi.mock('vue', async (importOriginal) => ({
  ...await importOriginal<typeof import('vue')>(),
  onMounted: (callback: () => void) => callback(),
  onUnmounted: (callback: () => void) => { lifecycle.unmount = callback },
}))

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { sendRequest, cancelRequest, historyRecord },
  normalizeError: (error: unknown) => ({
    kind: 'io',
    message: error instanceof Error ? error.message : String(error),
    hints: [],
  }),
  onRequestEvents: (handlers: typeof events.handlers) => {
    events.handlers = handlers
    return () => {}
  },
  toHistoryRequestPreview: (spec: unknown) => spec,
}))

import { useRequest } from './request'
import { useTabsStore } from './tabs'
import { useWorkspaceStore } from './workspace'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}

function prepareRequest() {
  const tabs = useTabsStore()
  tabs.open({ url: 'https://api.test/request', auth: { type: 'none' }, dirty: true })
  const workspace = useWorkspaceStore()
  workspace.ready = true
  workspace.workspaceId = 'workspace-id'
  workspace.sessionId = 'session-id'
  return { tabs, workspace, request: useRequest() }
}

function done(id: string) {
  const response = { id, status: 200 } as ResponseSummary
  events.handlers.onDone?.({ workspaceId: 'workspace-id', sessionId: 'session-id', response })
}

describe('request execution guard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.resetAllMocks()
    vi.useFakeTimers()
    cancelRequest.mockResolvedValue(true)
    historyRecord.mockResolvedValue(undefined)
    lifecycle.unmount = () => {}
    events.handlers = {}
  })

  afterEach(() => {
    lifecycle.unmount()
    vi.useRealTimers()
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

  it('keeps the original workspace session across environment saving', async () => {
    const { workspace, request } = prepareRequest()
    const saving = deferred<boolean>()
    vi.spyOn(workspace, 'saveDirtyEnvironments').mockReturnValueOnce(saving.promise)
    const sending = request.send()
    workspace.workspaceId = 'workspace-b'
    workspace.sessionId = 'session-b'
    saving.resolve(true)

    await sending

    expect(sendRequest).not.toHaveBeenCalled()
    expect(request.sending.value).toBe(false)
  })

  it('does not send if cancelled while waiting for environment persistence', async () => {
    const { workspace, request } = prepareRequest()
    const saving = deferred<boolean>()
    vi.spyOn(workspace, 'saveDirtyEnvironments').mockReturnValueOnce(saving.promise)
    const sending = request.send()
    request.cancel()
    saving.resolve(true)
    await sending

    expect(sendRequest).not.toHaveBeenCalled()
    expect(request.sending.value).toBe(false)
  })

  it('cancels a handle that arrives after cancellation without disturbing a newer request', async () => {
    const { request, tabs } = prepareRequest()
    const oldHandle = deferred<ExecHandle>()
    sendRequest.mockReturnValueOnce(oldHandle.promise).mockResolvedValueOnce({ id: 'new' })
    const oldSend = request.send()
    await vi.advanceTimersByTimeAsync(0)
    request.cancel()
    await request.send()

    oldHandle.resolve({ id: 'old' })
    await oldSend

    expect(cancelRequest).toHaveBeenCalledWith('old')
    expect(request.sending.value).toBe(true)
    done('old')
    expect(request.sending.value).toBe(true)
    expect(tabs.active?.response).toBeNull()
    done('new')
    expect(tabs.active?.response?.id).toBe('new')
    expect(request.sending.value).toBe(false)
  })

  it('ignores an old invocation failure after a newer request starts', async () => {
    const { request, tabs } = prepareRequest()
    const oldHandle = deferred<ExecHandle>()
    sendRequest.mockReturnValueOnce(oldHandle.promise).mockResolvedValueOnce({ id: 'new' })
    const oldSend = request.send()
    await vi.advanceTimersByTimeAsync(0)
    request.cancel()
    await request.send()
    oldHandle.reject(new Error('late failure'))
    await oldSend

    expect(request.sending.value).toBe(true)
    expect(tabs.active?.error).toBeNull()
    done('new')
    expect(tabs.active?.response?.id).toBe('new')
  })

  it('prevents duplicate sends while the workspace is still loading', async () => {
    const { workspace, request } = prepareRequest()
    workspace.ready = false
    const loading = deferred<void>()
    vi.spyOn(workspace, 'load').mockReturnValue(loading.promise)
    sendRequest.mockResolvedValue({ id: 'only-request' })
    const first = request.send()
    const second = request.send()
    expect(request.sending.value).toBe(true)
    workspace.ready = true
    loading.resolve()
    await Promise.all([first, second])

    expect(sendRequest).toHaveBeenCalledTimes(1)
  })

  it('applies an early completion without falsely marking an unsaved draft clean', async () => {
    const { request, tabs } = prepareRequest()
    const handle = deferred<ExecHandle>()
    sendRequest.mockReturnValueOnce(handle.promise)
    const sending = request.send()
    await vi.advanceTimersByTimeAsync(0)
    done('early')
    handle.resolve({ id: 'early' })
    await sending

    expect(tabs.active?.response?.id).toBe('early')
    expect(tabs.active?.dirty).toBe(true)
    expect(request.sending.value).toBe(false)
    expect(historyRecord).toHaveBeenCalledTimes(1)
  })

  it('abandons pending preparation when the request component unmounts', async () => {
    const { workspace, request } = prepareRequest()
    const saving = deferred<boolean>()
    vi.spyOn(workspace, 'saveDirtyEnvironments').mockReturnValueOnce(saving.promise)
    const sending = request.send()
    lifecycle.unmount()
    saving.resolve(true)
    await sending

    expect(sendRequest).not.toHaveBeenCalled()
  })

  it('cancels the active backend handle when the component unmounts', async () => {
    const { request } = prepareRequest()
    sendRequest.mockResolvedValueOnce({ id: 'active' })
    await request.send()

    lifecycle.unmount()

    expect(cancelRequest).toHaveBeenCalledWith('active')
    expect(request.sending.value).toBe(false)
  })

  it('does not revive a request whose handle arrives after the fallback timeout', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { request, tabs } = prepareRequest()
    const handle = deferred<ExecHandle>()
    sendRequest.mockReturnValueOnce(handle.promise)
    const sending = request.send()
    await vi.advanceTimersByTimeAsync(90_000)
    handle.resolve({ id: 'timed-out' })
    await sending
    done('timed-out')

    expect(cancelRequest).toHaveBeenCalledWith('timed-out')
    expect(request.sending.value).toBe(false)
    expect(tabs.active?.response).toBeNull()
  })
})
