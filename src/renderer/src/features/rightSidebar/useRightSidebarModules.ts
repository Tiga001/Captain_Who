import { useMemo } from 'react'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import { AGENT_CENTER_RIGHT_SIDEBAR_MODULE, RIGHT_SIDEBAR_MODULES } from './rightSidebarModules'
import type { RightSidebarModuleDefinition } from './rightSidebarTypes'

export function useRightSidebarModules({
  activeConversationId,
  collaborationSnapshot,
  configuredModules = RIGHT_SIDEBAR_MODULES
}: {
  activeConversationId?: string | null
  collaborationSnapshot?: CollaborationStoreSnapshot | null
  configuredModules?: RightSidebarModuleDefinition[]
}) {
  const childAgents = useMemo(() => {
    const tree = collaborationSnapshot?.tree
    if (!activeConversationId || tree?.rootConversationId !== activeConversationId) return []
    return tree.agents.filter((agent) => agent.parentAgentId !== null)
  }, [activeConversationId, collaborationSnapshot?.tree])
  const activeChildCount = childAgents.filter((agent) =>
    ['queued', 'running', 'waiting_approval'].includes(agent.displayStatus)
  ).length
  const modules = useMemo(() => {
    const withoutAgentCenter = configuredModules.filter((module) => module.id !== 'agent-center')
    return childAgents.length > 0
      ? [
          ...withoutAgentCenter,
          { ...AGENT_CENTER_RIGHT_SIDEBAR_MODULE, badge: activeChildCount || undefined }
        ]
      : withoutAgentCenter
  }, [activeChildCount, childAgents.length, configuredModules])
  return { childAgents, modules }
}
