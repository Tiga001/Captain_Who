import type { StorageComposerDraftRecord } from '@mycopilot/protocol'
import { beforeEach, expect, it, vi } from 'vitest'

const storage = vi.hoisted(() => ({
  loadComposerDrafts: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({
  hostClient: { storage }
}))

const { loadComposerDrafts } = await import('../../storage/storageClient')

function currentDraft(
  overrides: Partial<StorageComposerDraftRecord> = {}
): StorageComposerDraftRecord {
  return {
    scopeId: 'conversation-current',
    message: 'Continue the current task',
    permissionMode: 'default',
    permissionModeVersion: 1,
    modelId: null,
    projectId: null,
    attachmentsJson: '[]',
    skillsJson: '[]',
    queuedMessagesJson:
      '[{"id":"queued-1","clientMessageId":"client-1","content":"guide","attachments":[],"modelId":"model-1","permissionMode":"default","projectId":null,"skills":[],"status":"pending","createdAt":1}]',
    updatedAt: 1,
    ...overrides
  }
}

beforeEach(() => {
  storage.loadComposerDrafts.mockReset()
})

it('loads only the complete current composer draft shape', async () => {
  storage.loadComposerDrafts.mockResolvedValue([currentDraft()])

  await expect(loadComposerDrafts()).resolves.toMatchObject({
    'conversation-current': {
      modelId: '',
      projectId: null,
      queuedMessages: [{ id: 'queued-1', status: 'pending' }]
    }
  })
})

it.each([
  ['invalid JSON', { queuedMessagesJson: '{secret-provider-payload' }],
  ['non-array JSON', { attachmentsJson: '{}' }],
  [
    'missing queued fields',
    { queuedMessagesJson: '[{"id":"queued-1","clientMessageId":"client-1"}]' }
  ],
  [
    'extra attachment fields',
    {
      attachmentsJson:
        '[{"id":"attachment-1","kind":"file","name":"a.txt","sizeBytes":0,"encoding":"utf8","data":"","retiredField":true}]'
    }
  ]
])(
  'rejects a malformed persisted draft (%s) without exposing its payload',
  async (_label, fields) => {
    storage.loadComposerDrafts.mockResolvedValue([currentDraft(fields)])

    await expect(loadComposerDrafts()).rejects.toThrow('Stored composer draft is malformed')
  }
)
