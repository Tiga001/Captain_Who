import { useMemo } from 'react'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import { AGENT_CENTER_RIGHT_SIDEBAR_MODULE, RIGHT_SIDEBAR_MODULES } from './rightSidebarModules'
import {
  getRightSidebarModuleAvailability,
  resolveRightSidebarModuleAvailabilityMap
} from './rightSidebarModuleAvailability'
import type { RightSidebarCapabilities, RightSidebarModuleDefinition } from './rightSidebarTypes'
import { createRightSidebarWorkspaceContext } from './rightSidebarWorkspace'

const EMPTY_CAPABILITIES: RightSidebarCapabilities = {}

/** Shared by the module picker and external navigation entry points. */
export function useRightSidebarModuleAvailability({
  capabilities = EMPTY_CAPABILITIES,
  modules,
  workspaceKey,
  workspaceName,
  workspacePath
}: {
  capabilities?: RightSidebarCapabilities
  modules: RightSidebarModuleDefinition[]
  workspaceKey?: string | null
  workspaceName?: string | null
  workspacePath?: string
}) {
  const workspace = useMemo(
    () => createRightSidebarWorkspaceContext(workspaceKey, workspaceName, workspacePath),
    [workspaceKey, workspaceName, workspacePath]
  )
  const moduleAvailability = useMemo(
    () => resolveRightSidebarModuleAvailabilityMap(modules, capabilities, workspace),
    [capabilities, modules, workspace]
  )
  const availableModules = useMemo(
    () =>
      modules.filter(
        (module) => getRightSidebarModuleAvailability(moduleAvailability, module.id) === 'available'
      ),
    [moduleAvailability, modules]
  )
  return { availableModules, moduleAvailability, workspace }
}

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
