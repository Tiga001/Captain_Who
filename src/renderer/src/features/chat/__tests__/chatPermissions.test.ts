import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import type { AgentPermissions } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { resolveChatPermissions } from '../chatPermissions'
import type { ChatPermissionMode } from '../chatTypes'

const customPermissions: AgentPermissions = {
  read: 'all',
  write: 'denied',
  command: 'auto_approve',
  commandSafety: 'full_access',
  patch: 'require_approval',
  builtinExecution: 'auto_approve'
}

interface PermissionGoldenFixture {
  permissionGolden: {
    customPermissions: AgentPermissions
    resolved: Record<ChatPermissionMode, AgentPermissions>
  }
}

const permissionGolden = (
  JSON.parse(
    readFileSync(
      fileURLToPath(
        new URL(
          '../../../../../../packages/protocol/fixtures/automation-contract-v1.json',
          import.meta.url
        )
      ),
      'utf8'
    )
  ) as PermissionGoldenFixture
).permissionGolden

describe('resolveChatPermissions', () => {
  it('matches the Rust Automation resolver through the shared golden fixture', () => {
    for (const mode of ['default', 'full', 'custom'] as const) {
      expect(resolveChatPermissions(mode, permissionGolden.customPermissions)).toEqual(
        permissionGolden.resolved[mode]
      )
    }
  })

  it('keeps default and custom modes on the guarded command policy', () => {
    expect(resolveChatPermissions('default', customPermissions)).toEqual({
      read: 'workspace_only',
      write: 'workspace_only',
      command: 'require_approval',
      commandSafety: 'guarded',
      patch: 'require_approval',
      builtinExecution: 'require_approval'
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
      patch: 'auto_approve',
      builtinExecution: 'auto_approve'
    })
  })
})
