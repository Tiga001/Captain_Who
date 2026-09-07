import { describe, expect, it } from 'vitest'
import fixture from '../fixtures/conversation-context-material-v1.json'
import { parseConversationContextMaterial } from './agentContextMaterial'

const material = fixture.items[0]
describe('Host-authored context material contract', () => {
  it('preserves the exact model text, causal identity and immutable image references', () => {
    expect(fixture.schemaVersion).toBe(6)
    for (const item of fixture.items) expect(parseConversationContextMaterial(item)).toEqual(item)
  })
  it('rejects forged authority, binary data, malformed identity and non-attachment image fields', () => {
    for (const patch of [
      { eventId: '' },
      { eventId: 'a\nb' },
      { eventId: 'x'.repeat(513) },
      { sequence: -1 },
      { createdAt: -1 },
      { content: '', images: [] },
      { content: 'data:image/png;base64,AAAA' },
      { content: '界'.repeat(1_398_102) },
      { materialKind: 'system' },
      { materialKind: 'skill_instructions' },
      { approved: true },
      { runId: 'forged' },
      { images: [material.images![0], material.images![0]] },
      { images: [{ ...material.images![0], bytes: 'AAAA' }] },
      { images: [{ ...material.images![0], sha256: 'changed' }] },
      { images: [{ ...material.images![0], mimeType: 'text/plain' }] }
    ])
      expect(() => parseConversationContextMaterial({ ...material, ...patch })).toThrow()
  })
  it('accepts image-only attachment material and omits empty default image fields', () => {
    expect(parseConversationContextMaterial({ ...material, content: '' }).images).toEqual(
      material.images
    )
    expect(parseConversationContextMaterial({ ...fixture.items[1], images: [] })).toEqual(
      fixture.items[1]
    )
  })
})
