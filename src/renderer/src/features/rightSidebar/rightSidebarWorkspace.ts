import type { RightSidebarWorkspaceContext } from './rightSidebarTypes'

export function createRightSidebarWorkspaceSessionKey(
  workspaceKey: string | null | undefined,
  workspacePath: string | undefined
): string {
  return JSON.stringify([workspaceKey?.trim() || null, workspacePath?.trim() || null])
}

export function createRightSidebarWorkspaceContext(
  workspaceKey: string | null | undefined,
  workspaceName: string | null | undefined,
  workspacePath: string | undefined
): RightSidebarWorkspaceContext {
  const key = workspaceKey?.trim()
  const path = workspacePath?.trim() || undefined
  const pathName = path?.split(/[\\/]/).filter(Boolean).at(-1)?.trim()
  const name = workspaceName?.trim() || pathName || null

  return {
    hasWorkspace: Boolean(key || path),
    key: key || path || workspaceName?.trim() || 'home',
    name,
    path,
    sessionKey: createRightSidebarWorkspaceSessionKey(key, path)
  }
}
