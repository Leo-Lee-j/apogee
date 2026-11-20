import { expect, it, beforeEach, vi, afterEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import type { Body } from '@/ipc'
import { switchBody } from './bodyDrafts'
import { useTabsStore } from '@/stores/tabs'
vi.mock('@/ipc', () => ({ inTauri: false }))
beforeEach(() => setActivePinia(createPinia()))
afterEach(() => useTabsStore().resetWorkspace())
it('restores each independent type including form metadata, GraphQL fields and binary path', () => {
  const types: Body[] = [ { type: 'json', content: '{"fixture":true}' }, { type: 'text', content: '<fixture/>', contentType: 'application/xml' },
    { type: 'formData', rows: [{ key: 'file', value: 'fixture.txt', kind: 'file', enabled: false, description: 'keep' }] },
    { type: 'urlEncoded', rows: [{ key: 'x', value: 'one', enabled: false, description: 'keep' }] },
    { type: 'graphQl', query: 'query Q { id }', variables: '{"x":1}', operationName: 'Q' }, { type: 'binary', path: 'fixture.bin' } ]
  for (const body of types) {
    const none = switchBody(body, {}, 'none')
    const restored = switchBody(none.body, none.bodyDrafts, body.type)
    expect(restored.body).toEqual(body)
    expect(restored.bodyDrafts).not.toHaveProperty(body.type)
  }
  const initial = switchBody({ type: 'json', content: 'original' }, {}, 'text')
  expect(initial.body).toMatchObject({ content: 'original' })
  const back = switchBody({ type: 'text', content: 'different', contentType: 'application/xml' }, initial.bodyDrafts, 'json')
  expect(back.body).toEqual({ type: 'json', content: 'original' })
})
it('refuses lossy file conversion until a separate urlencoded draft exists', () => {
  const body: Body = { type: 'formData', rows: [{ enabled: true, kind: 'file', key: 'file', value: 'fixture' }] }
  expect(() => switchBody(body, {}, 'urlEncoded')).toThrow()
  expect(switchBody(body, { urlEncoded: { type: 'urlEncoded', rows: [] } }, 'urlEncoded').body).toEqual({ type: 'urlEncoded', rows: [] })
})
it('isolates tabs and preview identities, and rejects a switch exceeding the memory budget', () => {
  const tabs = useTabsStore()
  const first = tabs.open({ body: { type: 'json', content: 'one' } })
  tabs.switchBodyType(first.id, 'none')
  const second = tabs.open({ preview: true })
  tabs.switchBodyType(second.id, 'json')
  expect(tabs.active?.body).toEqual({ type: 'json', content: '' })
  tabs.activate(first.id); tabs.switchBodyType(first.id, 'json')
  expect(tabs.active?.body).toEqual({ type: 'json', content: 'one' })
  tabs.patch(first.id, { body: { type: 'text', content: 'x'.repeat(8 * 1024 * 1024), contentType: 'text/plain' } })
  expect(() => tabs.switchBodyType(first.id, 'none')).toThrow('8 MiB')
  expect(tabs.active?.body.type).toBe('text')
  const preview = tabs.open({ preview: true, bodyDrafts: { json: { type: 'json', content: 'old preview' } } })
  tabs.open({ preview: true, title: 'replacement' })
  expect(tabs.active?.id).toBe(preview.id)
  expect(tabs.active?.bodyDrafts).toEqual({})
})
