import type { AgentPermissions } from '@mycopilot/protocol'
import type { ChatPermissionMode } from './chatTypes'

const DEFAULT_PERMISSIONS: AgentPermissions = {
  read: 'workspace_only',
  write: 'workspace_only',
  command: 'require_approval',
  patch: 'require_approval'
}

const FULL_PERMISSIONS: AgentPermissions = {
  read: 'all',
  write: 'all',
  command: 'auto_approve',
  patch: 'auto_approve'
}

export function resolveChatPermissions(
  mode: ChatPermissionMode,
  customPermissions: AgentPermissions
): AgentPermissions {
  if (mode === 'full') return { ...FULL_PERMISSIONS }
  if (mode === 'custom') return { ...customPermissions }
  return { ...DEFAULT_PERMISSIONS }
}
