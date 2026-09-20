import { describe, expect, it, vi } from 'vitest'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))
import { createComposerDraft } from '../chatMessageFactory'
import { consumeSubmittedDraft, restoreRejectedDraft } from '../composerSubmission'

const submitted = createComposerDraft({
  message: 'original message',
  modelId: 'original-model',
  permissionMode: 'custom',
  projectId: 'original-project',
  skills: [{ id: 'original-skill', revision: 'v1' }],
  attachments: [
    {
      id: 'old',
      kind: 'file',
      name: 'old.txt',
      mimeType: 'text/plain',
      sizeBytes: 1,
      encoding: 'managed',
      data: 'import-old'
    }
  ],
  updatedAt: 1
})
const options = { ...submitted }

describe('composer submission ownership', () => {
  it('consumes the submitted version without mutating its recovery snapshot', () => {
    const consumed = consumeSubmittedDraft(submitted, submitted, options)
    expect(consumed).toMatchObject({ message: '', attachments: [], skills: [] })
    expect(submitted.message).toBe('original message')
    expect(restoreRejectedDraft(consumed, submitted, true, options)).toMatchObject({
      message: submitted.message,
      attachments: submitted.attachments,
      skills: submitted.skills,
      queuedMessages: []
    })
  })

  it('preserves a newer version even if the user typed the identical text', () => {
    const newer = { ...submitted, updatedAt: 2 }
    expect(consumeSubmittedDraft(newer, submitted, options)).toBe(newer)
  })

  it('puts the new draft in the queue and restores the old draft with both sets of settings', () => {
    const newer = createComposerDraft({
      message: 'new message',
      modelId: 'new-model',
      permissionMode: 'full',
      projectId: 'new-project',
      skills: [{ id: 'new-skill', revision: 'v2' }],
      attachments: [{ ...submitted.attachments[0], id: 'new', name: 'new.txt' }],
      updatedAt: 2
    })
    const restored = restoreRejectedDraft(newer, submitted, true, options)
    expect(restored).toMatchObject({
      message: submitted.message,
      modelId: submitted.modelId,
      permissionMode: submitted.permissionMode,
      projectId: submitted.projectId,
      attachments: submitted.attachments,
      skills: submitted.skills
    })
    expect(restored.queuedMessages).toEqual([
      expect.objectContaining({
        content: 'new message',
        modelId: newer.modelId,
        permissionMode: newer.permissionMode,
        projectId: newer.projectId,
        attachments: newer.attachments,
        skills: newer.skills,
        status: 'pending'
      })
    ])
  })

  it('retains an attachment-only new draft without creating a duplicate of an unconsumed old draft', () => {
    expect(restoreRejectedDraft(submitted, submitted, false, options).queuedMessages).toEqual([])
    const newer = { ...submitted, message: '', updatedAt: 2 }
    expect(
      restoreRejectedDraft(newer, submitted, false, options).queuedMessages[0].attachments
    ).toEqual(submitted.attachments)
  })
})
