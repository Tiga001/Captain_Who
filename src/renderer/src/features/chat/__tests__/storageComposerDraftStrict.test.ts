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
    permissionModeVersion: 2,
    modelId: null,
    projectId: null,
    attachmentsJson: '[]',
    folderReferencesJson: '[]',
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

it('restores managed references without loading file bytes', async () => {
  const attachments = [
    {
      id: 'managed',
      kind: 'file',
      name: 'large.csv',
      sizeBytes: 100_000_000,
      encoding: 'managed',
      data: 'opaque-import-id',
      contentSha256: `sha256:${'a'.repeat(64)}`
    }
  ]
  storage.loadComposerDrafts.mockResolvedValue([
    currentDraft({ attachmentsJson: JSON.stringify(attachments) })
  ])
  await expect(loadComposerDrafts()).resolves.toMatchObject({
    'conversation-current': { attachments }
  })
})

it('restores workspace mentions in queued messages', async () => {
  const workspaceMentions = [
    {
      id: 'primary:src/file.ts',
      projectId: 'project-1',
      folderId: 'primary',
      alias: 'app',
      displayName: 'file.ts',
      path: 'src/file.ts',
      displayPath: 'app/src/file.ts',
      kind: 'file'
    }
  ]
  storage.loadComposerDrafts.mockResolvedValue([
    currentDraft({
      queuedMessagesJson: JSON.stringify([
        {
          id: 'queued-1',
          clientMessageId: 'client-1',
          content: '[file.ts](@workspace/app/src/file.ts)',
          attachments: [],
          folderReferences: [],
          workspaceMentions,
          modelId: 'model-1',
          permissionMode: 'default',
          projectId: 'project-1',
          skills: [],
          status: 'pending',
          createdAt: 1
        }
      ])
    })
  ])

  await expect(loadComposerDrafts()).resolves.toMatchObject({
    'conversation-current': { queuedMessages: [{ workspaceMentions }] }
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
        '[{"id":"attachment-1","kind":"file","name":"a.txt","sizeBytes":0,"encoding":"managed","data":"opaque-id","retiredField":true}]'
    }
  ]
])(
  'rejects a malformed persisted draft (%s) without exposing its payload',
  async (_label, fields) => {
    storage.loadComposerDrafts.mockResolvedValue([currentDraft(fields)])

    await expect(loadComposerDrafts()).rejects.toThrow('Stored composer draft is malformed')
  }
)
