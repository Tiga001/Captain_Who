import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'

interface WorkspaceFileTreeSessionResource {
  destroy: () => void
}

class WorkspaceFileTreeSessionRegistry {
  private destroyTimer: ReturnType<typeof setTimeout> | null = null
  private readonly sessions = new Map<string, WorkspaceFileTreeSessionResource>()

  getSession<TSession extends WorkspaceFileTreeSessionResource>(
    projectId: string,
    revision: string,
    createSession: () => TSession
  ): TSession {
    const key = JSON.stringify([projectId, revision])
    const existingSession = this.sessions.get(key)
    if (existingSession) return existingSession as TSession

    const session = createSession()
    this.sessions.set(key, session)
    return session
  }

  reconcile(projectIds: readonly string[], revisions: Readonly<Record<string, string>>): void {
    const retainedProjectIds = new Set(
      projectIds.map((id) => JSON.stringify([id, revisions[id] ?? '']))
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
  registry: WorkspaceFileTreeSessionRegistry
  revisions: Readonly<Record<string, string>>
} | null>(null)
const EMPTY_REVISIONS: Readonly<Record<string, string>> = {}

interface WorkspaceFileTreeSessionsProviderProps {
  children: ReactNode
  projectIds: readonly string[]
  projectRevisions?: Readonly<Record<string, string>>
}

export function WorkspaceFileTreeSessionsProvider({
  children,
  projectIds,
  projectRevisions = EMPTY_REVISIONS
}: WorkspaceFileTreeSessionsProviderProps): ReactNode {
  const [registry] = useState(() => new WorkspaceFileTreeSessionRegistry())

  useLayoutEffect(() => {
    registry.reconcile(projectIds, projectRevisions)
  }, [projectIds, projectRevisions, registry])

  useEffect(() => {
    registry.cancelScheduledDestroy()
    return () => registry.scheduleDestroy()
  }, [registry])

  const value = useMemo(
    () => ({ registry, revisions: projectRevisions }),
    [registry, projectRevisions]
  )
  return (
    <WorkspaceFileTreeSessionsContext.Provider value={value}>
      {children}
    </WorkspaceFileTreeSessionsContext.Provider>
  )
}

export function useWorkspaceFileTreeSessionResource<
  TSession extends WorkspaceFileTreeSessionResource
>(projectId: string, createSession: () => TSession): TSession {
  const context = useContext(WorkspaceFileTreeSessionsContext)
  if (!context) {
    throw new Error('FilesPanel must be rendered inside WorkspaceFileTreeSessionsProvider')
  }
  return context.registry.getSession(projectId, context.revisions[projectId] ?? '', createSession)
}

export function useWorkspaceFileRevision(projectId: string): string {
  return useContext(WorkspaceFileTreeSessionsContext)?.revisions[projectId] ?? ''
}
