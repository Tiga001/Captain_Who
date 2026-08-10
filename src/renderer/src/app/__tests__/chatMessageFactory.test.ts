import type { ChatPermissionMode } from '../../features/chat/chatTypes'
import { describe, expect, it } from 'vitest'
import {
  createComposerDraft,
  createForkComposerDraft,
  synchronizeComposerDraftForScope
} from '../chatMessageFactory'

describe('createComposerDraft', () => {
  it('uses default permission mode for a new composer', () => {
    expect(createComposerDraft().permissionMode).toBe('default')
  })

  it.each<ChatPermissionMode>(['default', 'custom', 'full'])(
    'preserves the supported %s permission mode',
    (permissionMode) => {
      expect(createComposerDraft({ permissionMode }).permissionMode).toBe(permissionMode)
    }
  )

  it('falls back to default permission mode for stale persisted values', () => {
    expect(
      createComposerDraft({ permissionMode: 'legacy-full' as ChatPermissionMode }).permissionMode
    ).toBe('default')
  })

  it('uses the authoritative ref snapshot when message-only updates have not rerendered AppShell', () => {
    const rendered = createComposerDraft({ message: '', updatedAt: 1 })
    const authoritative = createComposerDraft({ message: '跨对话保留的草稿', updatedAt: 2 })

    const synchronized = synchronizeComposerDraftForScope(
      { conversation: rendered },
      { conversation: authoritative },
      'conversation'
    )

    expect(synchronized.conversation).toBe(authoritative)
  })

  it('keeps the source composer model independent from the forked history model', () => {
    const sourceDraft = createComposerDraft({
      modelId: 'deepseek-draft-model',
      permissionMode: 'custom',
      projectId: 'source-project'
    })

    const forkDraft = createForkComposerDraft(sourceDraft, {
      modelId: 'generic-history-model',
      projectId: 'fork-project'
    })

    expect(forkDraft).toMatchObject({
      message: '',
      modelId: 'deepseek-draft-model',
      permissionMode: 'custom',
      projectId: 'fork-project'
    })
  })
})
