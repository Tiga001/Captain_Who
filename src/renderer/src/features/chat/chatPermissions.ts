import type { AgentPermissions } from '@mycopilot/protocol'
import type { ChatPermissionMode } from './chatTypes'

const DEFAULT_PERMISSIONS: AgentPermissions = {
  read: 'workspace_only',
  write: 'workspace_only',
  command: 'require_approval',
  commandSafety: 'guarded',
  patch: 'require_approval'
}

const FULL_PERMISSIONS: AgentPermissions = {
  read: 'all',
  write: 'all',
  command: 'auto_approve',
  commandSafety: 'full_access',
  patch: 'auto_approve'
}

export function resolveChatPermissions(
  mode: ChatPermissionMode,
  customPermissions: AgentPermissions
): AgentPermissions {
  if (mode === 'full') return { ...FULL_PERMISSIONS }
  if (mode === 'custom') return { ...customPermissions, commandSafety: 'guarded' }
  return { ...DEFAULT_PERMISSIONS }
}
