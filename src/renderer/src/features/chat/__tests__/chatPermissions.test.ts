import type { AgentPermissions } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { resolveChatPermissions } from '../chatPermissions'

const customPermissions: AgentPermissions = {
  read: 'all',
  write: 'denied',
  command: 'auto_approve',
  commandSafety: 'full_access',
  patch: 'require_approval'
}

describe('resolveChatPermissions', () => {
  it('keeps default and custom modes on the guarded command policy', () => {
    expect(resolveChatPermissions('default', customPermissions)).toEqual({
      read: 'workspace_only',
      write: 'workspace_only',
      command: 'require_approval',
      commandSafety: 'guarded',
      patch: 'require_approval'
    })
    expect(resolveChatPermissions('custom', customPermissions)).toEqual({
      ...customPermissions,
      commandSafety: 'guarded'
    })
  })

  it('uses full-access command safety only for full permission mode', () => {
    expect(resolveChatPermissions('full', customPermissions)).toEqual({
      read: 'all',
      write: 'all',
      command: 'auto_approve',
      commandSafety: 'full_access',
      patch: 'auto_approve'
    })
  })
})
