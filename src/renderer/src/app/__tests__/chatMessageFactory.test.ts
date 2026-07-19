import type { ChatPermissionMode } from '../../features/chat/chatTypes'
import { describe, expect, it } from 'vitest'
import { createComposerDraft } from '../chatMessageFactory'

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
})
