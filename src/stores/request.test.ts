import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

const { sendRequest } = vi.hoisted(() => ({
  sendRequest: vi.fn(),
}))

vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { sendRequest },
  normalizeError: (error: unknown) => ({
    kind: 'io',
    message: error instanceof Error ? error.message : String(error),
    hints: [],
  }),
  onRequestEvents: () => () => {},
}))

import { useRequest } from './request'
import { useTabsStore } from './tabs'
import { useWorkspaceStore } from './workspace'

describe('request execution guard', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    sendRequest.mockReset()
  })

  afterEach(() => {
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
})
