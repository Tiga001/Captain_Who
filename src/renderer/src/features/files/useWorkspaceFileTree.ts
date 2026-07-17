import { useLayoutEffect, useState, useSyncExternalStore } from 'react'
import { useWorkspaceFileTreeSessionResource } from './WorkspaceFileTreeSessions'
import { WorkspaceFileTreeSession } from './workspaceFileTreeSession'

const INACTIVE_CONSUMER = {
  isActive: false,
  onFileSelect: () => undefined,
  selectedPath: null
}

interface UseWorkspaceFileTreeOptions {
  isActive: boolean
  onFileSelect: (path: string) => void
  projectId: string
  selectedPath: string | null
}

export function useWorkspaceFileTree({
  isActive,
  onFileSelect,
  projectId,
  selectedPath
}: UseWorkspaceFileTreeOptions) {
  const session = useWorkspaceFileTreeSessionResource(
    projectId,
    () => new WorkspaceFileTreeSession(projectId)
  )
  const [consumerToken] = useState(() => Symbol(`workspace-file-tree:${projectId}`))
  const snapshot = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot)

  useLayoutEffect(() => {
    session.attachConsumer(consumerToken, INACTIVE_CONSUMER)
    return () => session.detachConsumer(consumerToken)
  }, [consumerToken, session])

  useLayoutEffect(() => {
    session.updateConsumer(consumerToken, { isActive, onFileSelect, selectedPath })
  }, [consumerToken, isActive, onFileSelect, selectedPath, session])

  return {
    ...snapshot,
    model: session.model,
    refresh: session.refresh,
    setSearchQuery: session.setSearchQuery,
    setTreeVisible: session.setTreeVisible
  }
}
