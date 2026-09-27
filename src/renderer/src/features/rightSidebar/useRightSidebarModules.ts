import { useMemo } from 'react'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import {
  AGENT_CENTER_RIGHT_SIDEBAR_MODULE,
  RIGHT_SIDEBAR_MODULES,
  WORKFLOWS_RIGHT_SIDEBAR_MODULE
} from './rightSidebarModules'
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
  configuredModules = RIGHT_SIDEBAR_MODULES,
  workflowEnabled = false
}: {
  activeConversationId?: string | null
  collaborationSnapshot?: CollaborationStoreSnapshot | null
  configuredModules?: RightSidebarModuleDefinition[]
  workflowEnabled?: boolean
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
    const withoutAgentCenter = configuredModules.filter(
      (module) => module.id !== 'agent-center' && module.id !== 'workflows'
    )
    if (workflowEnabled) withoutAgentCenter.push(WORKFLOWS_RIGHT_SIDEBAR_MODULE)
    return childAgents.length > 0
      ? [
          ...withoutAgentCenter,
          { ...AGENT_CENTER_RIGHT_SIDEBAR_MODULE, badge: activeChildCount || undefined }
        ]
      : withoutAgentCenter
  }, [activeChildCount, childAgents.length, configuredModules, workflowEnabled])
  return { childAgents, modules }
}
