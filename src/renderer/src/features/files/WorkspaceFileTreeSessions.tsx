import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { primaryProjectFolder, sortedProjectFolders } from '../../config/projectConfig'
import type { AppProject, AppProjectFolder } from '../../config/projectConfig'

interface WorkspaceFileTreeSessionResource {
  destroy: () => void
}

class WorkspaceFileTreeSessionRegistry {
  private destroyTimer: ReturnType<typeof setTimeout> | null = null
  private readonly sessions = new Map<string, WorkspaceFileTreeSessionResource>()

  getSession<TSession extends WorkspaceFileTreeSessionResource>(
    projectId: string,
    folderId: string | undefined,
    revision: string,
    createSession: () => TSession
  ): TSession {
    const key = JSON.stringify([projectId, folderId ?? null, revision])
    const existingSession = this.sessions.get(key)
    if (existingSession) return existingSession as TSession

    const session = createSession()
    this.sessions.set(key, session)
    return session
  }

  reconcile(
    projectIds: readonly string[],
    revisions: Readonly<Record<string, string>>,
    projects: readonly AppProject[]
  ): void {
    const retainedProjectIds = new Set(
      projectIds.flatMap((id) => {
        const folders = projects.find((project) => project.id === id)?.folders
        return folders?.length
          ? folders.map((folder) => JSON.stringify([id, folder.id, folderRevision(folder)]))
          : [JSON.stringify([id, null, revisions[id] ?? ''])]
      })
    )
    for (const [projectId, session] of this.sessions) {
      if (retainedProjectIds.has(projectId)) continue
      session.destroy()
      this.sessions.delete(projectId)
    }
  }

  cancelScheduledDestroy(): void {
    if (this.destroyTimer === null) return
    clearTimeout(this.destroyTimer)
    this.destroyTimer = null
  }

  scheduleDestroy(): void {
    this.cancelScheduledDestroy()
    this.destroyTimer = setTimeout(() => {
      this.destroyTimer = null
      for (const session of this.sessions.values()) session.destroy()
      this.sessions.clear()
    }, 0)
  }
}

const WorkspaceFileTreeSessionsContext = createContext<{
  projects: readonly AppProject[]
  registry: WorkspaceFileTreeSessionRegistry
  revisions: Readonly<Record<string, string>>
  selectedFolders: Readonly<Record<string, string>>
  selectFolder: (projectId: string, folderId: string) => void
} | null>(null)
const EMPTY_REVISIONS: Readonly<Record<string, string>> = {}
const EMPTY_PROJECTS: readonly AppProject[] = []

interface WorkspaceFileTreeSessionsProviderProps {
  children: ReactNode
  projectIds: readonly string[]
  projectRevisions?: Readonly<Record<string, string>>
  projects?: readonly AppProject[]
}

export function WorkspaceFileTreeSessionsProvider({
  children,
  projectIds,
  projectRevisions = EMPTY_REVISIONS,
  projects = EMPTY_PROJECTS
}: WorkspaceFileTreeSessionsProviderProps): ReactNode {
  const [registry] = useState(() => new WorkspaceFileTreeSessionRegistry())
  const [selectedFolders, setSelectedFolders] = useState<Record<string, string>>({})

  useLayoutEffect(() => {
    registry.reconcile(projectIds, projectRevisions, projects)
  }, [projectIds, projectRevisions, projects, registry])

  useEffect(() => {
    registry.cancelScheduledDestroy()
    return () => registry.scheduleDestroy()
  }, [registry])

  const value = useMemo(
    () => ({
      projects,
      registry,
      revisions: projectRevisions,
      selectedFolders,
      selectFolder: (projectId: string, folderId: string) => {
        if (
          !projects
            .find((project) => project.id === projectId)
            ?.folders.some((folder) => folder.id === folderId)
        )
          return
        setSelectedFolders((current) =>
          current[projectId] === folderId ? current : { ...current, [projectId]: folderId }
        )
      }
    }),
    [registry, projectRevisions, projects, selectedFolders]
  )
  return (
    <WorkspaceFileTreeSessionsContext.Provider value={value}>
      {children}
    </WorkspaceFileTreeSessionsContext.Provider>
  )
}

export function useWorkspaceFileTreeSessionResource<
  TSession extends WorkspaceFileTreeSessionResource
>(projectId: string, folderId: string | undefined, createSession: () => TSession): TSession {
  const context = useContext(WorkspaceFileTreeSessionsContext)
  if (!context) {
    throw new Error('FilesPanel must be rendered inside WorkspaceFileTreeSessionsProvider')
  }
  const folder = context.projects
    .find((project) => project.id === projectId)
    ?.folders.find((folder) => folder.id === folderId)
  return context.registry.getSession(
    projectId,
    folderId,
    folder ? folderRevision(folder) : (context.revisions[projectId] ?? ''),
    createSession
  )
}

export function useWorkspaceFileRevision(projectId: string, folderId?: string): string {
  const context = useContext(WorkspaceFileTreeSessionsContext)
  const folder = context?.projects
    .find((project) => project.id === projectId)
    ?.folders.find((folder) => folder.id === folderId)
  return folder ? folderRevision(folder) : (context?.revisions[projectId] ?? '')
}

/** Tree browsing is independent from the source identity of each open file page. */
export function useWorkspaceFileRoot(projectId: string) {
  const context = useContext(WorkspaceFileTreeSessionsContext)
  const project = context?.projects.find((project) => project.id === projectId)
  const folders = sortedProjectFolders(project)
  const primaryFolderId = primaryProjectFolder(project)?.id
  const selectedId = context?.selectedFolders[projectId]
  const folderId = folders.find((folder) => folder.id === selectedId)?.id ?? primaryFolderId
  return {
    folders,
    folderId,
    primaryFolderId,
    selectFolder: (nextFolderId: string) => context?.selectFolder(projectId, nextFolderId)
  }
}

function folderRevision(folder: AppProjectFolder): string {
  // Changing a role or an alias does not invalidate directory data bound to the same root.
  return JSON.stringify([folder.id, folder.path])
}
