import type { Body } from '@/ipc'
import { convertFormBody } from './requestParameters'

export type BodyDrafts = Partial<Record<Body['type'], Body>>
export const BODY_DRAFT_BYTES = 8 * 1024 * 1024

/** Existing type drafts win; conversion is only the initial value of a new type. */
export function switchBody(body: Body, drafts: BodyDrafts, type: Body['type']) {
  const next = { ...drafts, [body.type]: body }
  let value = next[type]
  if (!value) {
    const content = 'content' in body ? body.content : ''
    if (type === 'formData' || type === 'urlEncoded') value = convertFormBody(body, type)
    else if (type === 'json') value = { type, content }
    else if (type === 'text') value = { type, content, contentType: 'text/plain' }
    else if (type === 'binary') value = { type, path: '' }
    else if (type === 'graphQl') value = { type, query: '', variables: '{}', operationName: undefined }
    else value = { type: 'none' }
  }
  delete next[type]
  return { body: value, bodyDrafts: next }
}

export function bodyDraftBytes(drafts: BodyDrafts) {
  return new TextEncoder().encode(JSON.stringify(drafts)).byteLength
}
