import type { ChatPermissionMode } from '../chat/chatTypes'

/**
 * Version of the permission semantics presented by this Renderer. Persisting the version makes
 * an explicit choice distinguishable from a legacy string whose meaning has since changed.
 */
export const CURRENT_COMPOSER_PERMISSION_MODE_VERSION = 1

export function serializeComposerPermissionMode(permissionMode: ChatPermissionMode): {
  permissionMode: ChatPermissionMode
  permissionModeVersion: number
} {
  return {
    permissionMode,
    permissionModeVersion: CURRENT_COMPOSER_PERMISSION_MODE_VERSION
  }
}

export function normalizeStoredComposerPermissionMode(
  permissionMode: string,
  permissionModeVersion: number | null | undefined
): ChatPermissionMode {
  if (permissionMode === 'full') {
    return permissionModeVersion === CURRENT_COMPOSER_PERMISSION_MODE_VERSION ? 'full' : 'default'
  }

  return permissionMode === 'custom' ? 'custom' : 'default'
}
