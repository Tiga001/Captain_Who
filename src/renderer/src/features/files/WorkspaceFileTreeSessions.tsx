import { createContext, useContext, useEffect, useLayoutEffect, useState } from 'react'
import type { ReactNode } from 'react'

interface WorkspaceFileTreeSessionResource {
  destroy: () => void
}

class WorkspaceFileTreeSessionRegistry {
  private destroyTimer: ReturnType<typeof setTimeout> | null = null
  private readonly sessions = new Map<string, WorkspaceFileTreeSessionResource>()

  getSession<TSession extends WorkspaceFileTreeSessionResource>(
    projectId: string,
    createSession: () => TSession
  ): TSession {
    const existingSession = this.sessions.get(projectId)
    if (existingSession) return existingSession as TSession

    const session = createSession()
    this.sessions.set(projectId, session)
    return session
  }

  reconcile(projectIds: readonly string[]): void {
    const retainedProjectIds = new Set(projectIds)
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

const WorkspaceFileTreeSessionsContext = createContext<WorkspaceFileTreeSessionRegistry | null>(
  null
)

interface WorkspaceFileTreeSessionsProviderProps {
  children: ReactNode
  projectIds: readonly string[]
}

export function WorkspaceFileTreeSessionsProvider({
  children,
  projectIds
}: WorkspaceFileTreeSessionsProviderProps): ReactNode {
  const [registry] = useState(() => new WorkspaceFileTreeSessionRegistry())

  useLayoutEffect(() => {
    registry.reconcile(projectIds)
  }, [projectIds, registry])

  useEffect(() => {
    registry.cancelScheduledDestroy()
    return () => registry.scheduleDestroy()
  }, [registry])

  return (
    <WorkspaceFileTreeSessionsContext.Provider value={registry}>
      {children}
    </WorkspaceFileTreeSessionsContext.Provider>
  )
}

export function useWorkspaceFileTreeSessionResource<
  TSession extends WorkspaceFileTreeSessionResource
>(projectId: string, createSession: () => TSession): TSession {
  const registry = useContext(WorkspaceFileTreeSessionsContext)
  if (!registry) {
    throw new Error('FilesPanel must be rendered inside WorkspaceFileTreeSessionsProvider')
  }
  return registry.getSession(projectId, createSession)
}
