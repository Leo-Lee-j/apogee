import { expect, it } from 'vitest'
import type { RequestSpec } from '@/ipc'
import { postmanRequest } from './postmanExport'
it('exports file rows, content type, path values and explicit environment URL templates', () => {
  const spec: RequestSpec = { method: 'POST', url: '{{env:apiBase}}/users/{{id}}?copy={id}', auth: { type: 'none' }, settings: {}, headers: [], params: [],
    pathParams: [{ enabled: true, key: 'id', value: '42' }],
    body: { type: 'formData', rows: [{ enabled: false, key: 'upload', value: 'fixture.txt', kind: 'file', description: 'keep' }] },
  }
  const exported = postmanRequest(spec)
  expect(exported.url).toEqual({ raw: '{{apiBase}}/users/:id?copy={id}', variable: [{ key: 'id', value: '42', disabled: false }] })
  expect(exported.body).toEqual({ mode: 'formdata', formdata: [{ key: 'upload', src: 'fixture.txt', type: 'file', disabled: true, description: 'keep' }] })
  spec.body = { type: 'text', content: '<fixture/>', contentType: 'application/xml' }
  expect(postmanRequest(spec).header).toEqual([{ key: 'Content-Type', value: 'application/xml', disabled: false }])
})
