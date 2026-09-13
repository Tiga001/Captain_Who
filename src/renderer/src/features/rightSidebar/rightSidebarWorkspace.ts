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
  const projectId = workspaceKey?.trim() || null
  const path = workspacePath?.trim() || undefined
  const pathName = path?.split(/[\\/]/).filter(Boolean).at(-1)?.trim()
  const name = workspaceName?.trim() || pathName || null

  return {
    hasWorkspace: Boolean(projectId || path),
    // This key is also used for page grouping and labels, so a no-project terminal still has
    // stable UI identity. It is deliberately distinct from `projectId`.
    key: projectId || path || workspaceName?.trim() || 'home',
    name,
    path,
    projectId,
    sessionKey: createRightSidebarWorkspaceSessionKey(projectId, path)
  }
}
