import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { AuthConfig } from '@/ipc'
import { useTabsStore } from './tabs'

const { saveRequest } = vi.hoisted(() => ({ saveRequest: vi.fn() }))
vi.mock('@/ipc', () => ({
  inTauri: true,
  ipc: { workspaceSaveRequest: saveRequest },
  normalizeError: (error: unknown) => ({ kind: 'io', message: error instanceof Error ? error.message : String(error), hints: [] }),
}))
const oldAuth: AuthConfig = { type: 'oauth2', accessToken: 'old', headerPrefix: 'Bearer' }
const rotated: AuthConfig = { type: 'oauth2', accessToken: 'rotated', headerPrefix: 'Bearer', tokenContext: 'private-context' }

beforeEach(() => { setActivePinia(createPinia()); vi.useFakeTimers(); saveRequest.mockReset().mockResolvedValue(undefined) })
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers() })

describe('saved Auth optimistic concurrency', () => {
  it('passes the persisted baseline, not the edited Auth, and keeps it out of RequestSpec', async () => {
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', auth: oldAuth })
    tabs.patch(tab.id, { auth: rotated, dirty: true })
    expect(await tabs.save(tab.id)).toEqual({ ok: true, error: null })
    expect(saveRequest).toHaveBeenCalledWith('request', tab.title, expect.objectContaining({ auth: rotated }), oldAuth)
    expect(saveRequest.mock.calls[0][2]).not.toHaveProperty('persistedAuth')
    expect(tabs.active?.persistedAuth).toBe(rotated)
  })

  it('does not let a late normal-save completion replace a newer credential baseline', async () => {
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', auth: oldAuth })
    let finish!: () => void
    saveRequest.mockImplementation(() => new Promise<void>(resolve => { finish = resolve }))
    const pending = tabs.save(tab.id)
    tabs.patch(tab.id, { auth: rotated, persistedAuth: rotated })
    finish()
    await pending
    expect(tabs.active?.persistedAuth).toBe(rotated)
    expect(tabs.active?.auth).toBe(rotated)
  })

  it('keeps unsaved edits and baseline when the host rejects a stale save', async () => {
    const tabs = useTabsStore()
    const tab = tabs.open({ nodeId: 'request', auth: oldAuth })
    tabs.patch(tab.id, { auth: rotated, dirty: true })
    saveRequest.mockRejectedValue(new Error('AUTH_CONFIG_STALE'))
    const result = await tabs.save(tab.id)
    expect(result.ok).toBe(false)
    expect(result.error?.message).toContain('未覆盖新凭据')
    expect(tabs.active?.error?.message).toContain('未覆盖新凭据')
    expect(tabs.active?.dirty).toBe(true)
    expect(tabs.active?.auth).toBe(rotated)
    expect(tabs.active?.persistedAuth).toBe(oldAuth)
    expect(saveRequest).toHaveBeenCalledOnce()
  })
})
