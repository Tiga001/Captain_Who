import { createContext, useContext } from 'react'
import type { ReactNode } from 'react'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import type { AgentObserverRenderContext } from './rightSidebarTypes'
import type { AppProject } from '../../config/projectConfig'

interface RightSidebarRuntimeContextValue {
  projects?: readonly AppProject[]
  onBrowserAutomationTargetChange?: (
    surfaceId: string,
    surfaceInstanceId: string,
    isTarget: boolean
  ) => void
  activeConversationId: string | null
  activeWorkspaceKey: string | null
  collaborationSnapshot: CollaborationStoreSnapshot | null
  browserSurfaceRequest?: { pageId: string; requestId: string }
  onBrowserSurfaceInstance?: (
    pageId: string,
    surfaceId: string,
    surfaceInstanceId: string,
    isCurrent: boolean
  ) => void
  onBrowserSurfaceReady?: (
    pageId: string,
    surfaceId: string,
    requestId: string,
    surfaceInstanceId: string,
    viewport?: { height: number; width: number }
  ) => void
  onOpenAgentTemplates?: () => void
  onOpenProfile?: () => void
  onOpenAgentRootConversation?: (conversationId: string) => void
  onOpenBrowserSettings?: (destination: 'settings' | 'downloads' | 'history') => void
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
