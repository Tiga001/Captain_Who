import { createContext, useContext } from 'react'
import type { ReactNode } from 'react'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import type { AgentObserverRenderContext } from './rightSidebarTypes'

interface RightSidebarRuntimeContextValue {
  activeConversationId: string | null
  activeWorkspaceKey: string | null
  collaborationSnapshot: CollaborationStoreSnapshot | null
  browserSurfaceRequest?: { pageId: string; requestId: string }
  onBrowserSurfaceReady?: (pageId: string, surfaceId: string, requestId: string) => void
  onOpenAgentTemplates?: () => void
  renderAgentObserver?: (context: AgentObserverRenderContext) => ReactNode
}

export const RightSidebarRuntimeContext = createContext<RightSidebarRuntimeContextValue>({
  activeConversationId: null,
  activeWorkspaceKey: null,
  collaborationSnapshot: null
})

export function useRightSidebarRuntimeContext(): RightSidebarRuntimeContextValue {
  return useContext(RightSidebarRuntimeContext)
}
