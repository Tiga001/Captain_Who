import { expect, it } from 'vitest'
import {
  pastedTextMetadata,
  restorePastedText,
  PASTED_TEXT_ATTACHMENT_THRESHOLD
} from '../../features/chat/pastedText'
import { createComposerDraft, createUserMessage } from '../chatMessageFactory'
import {
  mapDraftFromStorage,
  mapDraftToStorage
} from '../../features/storage/composerDraftPersistence'
import { parseTimelineItem } from '../../features/storage/persistedAgentRunTimelineValidators'
import { guidanceAttachments } from '../../features/agentRun/agentEventReducerGuidance'
import type { AgentInputAttachment } from '@mycopilot/protocol'

it('uses UTF-16 units for the threshold and character count while retaining whole Unicode preview characters', () => {
  expect(PASTED_TEXT_ATTACHMENT_THRESHOLD).toBe(5000)
  expect('🙂'.repeat(2500).length).toBe(PASTED_TEXT_ATTACHMENT_THRESHOLD)
  const text = `${'a'.repeat(79)}🙂 after\r\n\tmore`
  expect(pastedTextMetadata(text)).toEqual({
    preview: `${'a'.repeat(79)}🙂`,
    characterCount: text.length
  })
  expect(pastedTextMetadata(' \n Hello \t world\r\n')).toEqual({
    preview: 'Hello world',
    characterCount: 18
  })
})

it('restores full text into a selection and follows unrelated edits during loading', () => {
  const text = '中文🙂\r\n'.repeat(10_000)
  expect(restorePastedText('prefix OLD suffix', text, { start: 7, end: 10 })).toEqual({
    message: `prefix ${text} suffix`,
    cursor: 7 + text.length
  })
  expect(
    restorePastedText('new prefix OLD suffix', text, { start: 7, end: 10 }, 'prefix OLD suffix')
  ).toEqual({ message: `new prefix ${text} suffix`, cursor: 11 + text.length })
  expect(
    restorePastedText('prefix OLD suffix typed', text, { start: 7, end: 10 }, 'prefix OLD suffix')
  ).toEqual({ message: `prefix ${text} suffix typed`, cursor: 7 + text.length })
})

it('keeps newer typing inside an old selection when full text arrives later', () => {
  expect(
    restorePastedText('before NEW after', 'PASTED', { start: 7, end: 10 }, 'before OLD after')
  ).toEqual({ message: 'before NEWPASTED after', cursor: 16 })
})

it('round-trips compact pasted metadata through draft, queue, optimistic and guidance messages', () => {
  const pastedText = pastedTextMetadata('🙂'.repeat(5000))!
  const attachment: AgentInputAttachment = {
    id: 'pasted',
    kind: 'file',
    name: '粘贴的文本.txt',
    mimeType: 'text/plain',
    sizeBytes: 20_000,
    encoding: 'managed',
    data: 'opaque-id',
    pastedText
  }
  const draft = createComposerDraft({
    attachments: [attachment],
    queuedMessages: [
      {
        id: 'queued',
        clientMessageId: 'client',
        content: 'guide',
        attachments: [attachment],
        modelId: 'model',
        permissionMode: 'default',
        projectId: null,
        skills: [],
        status: 'pending',
        createdAt: 1
      }
    ]
  })
  const stored = mapDraftToStorage('scope', draft)
  expect(stored.attachmentsJson.length).toBeLessThan(500)
  const restored = mapDraftFromStorage(stored)
  expect(restored.attachments).toEqual([attachment])
  expect(restored.queuedMessages[0].attachments).toEqual([attachment])
  expect(createUserMessage('message', [attachment]).attachments?.[0].pastedText).toEqual(pastedText)
  const guidance = guidanceAttachments([attachment])
  expect(guidance[0].pastedText).toEqual(pastedText)
  const timeline = {
    id: 'guidance',
    type: 'user_guidance',
    clientMessageId: 'client',
    content: '',
    attachments: guidance,
    status: 'applied',
    createdAt: 1
  }
  expect(parseTimelineItem(timeline)).toEqual(timeline)
})

it.each([
  { preview: 'x'.repeat(81), characterCount: 5000 },
  { preview: 'text', characterCount: -1 },
  { preview: 'text', characterCount: 5000, fullText: 'must not be stored' }
])('rejects malformed pasted metadata in stored drafts', (pastedText) => {
  const draft = mapDraftToStorage('scope', createComposerDraft())
  draft.attachmentsJson = JSON.stringify([
    {
      id: 'paste',
      kind: 'file',
      name: 'text.txt',
      sizeBytes: 5000,
      encoding: 'managed',
      data: 'opaque',
      pastedText
    }
  ])
  expect(() => mapDraftFromStorage(draft)).toThrow('Stored composer draft is malformed')
})
