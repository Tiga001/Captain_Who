import type {
  RightSidebarCapabilities,
  RightSidebarModuleAvailability,
  RightSidebarModuleAvailabilityMap,
  RightSidebarModuleDefinition,
  RightSidebarPage,
  RightSidebarWorkspaceContext
} from './rightSidebarTypes'

export function resolveRightSidebarModuleAvailability(
  module: RightSidebarModuleDefinition,
  capabilities: RightSidebarCapabilities,
  workspace: RightSidebarWorkspaceContext
): RightSidebarModuleAvailability {
  if (module.requiresWorkspace && !workspace.hasWorkspace) return 'unavailable'
  if (!module.requiredCapability) return 'available'
  if (!workspace.hasWorkspace) return 'unavailable'

  const capability = capabilities[module.requiredCapability]
  if (!capability || capability.contextKey !== workspace.sessionKey) return 'checking'
  return capability.status
}

export function resolveRightSidebarModuleAvailabilityMap(
  modules: RightSidebarModuleDefinition[],
  capabilities: RightSidebarCapabilities,
  workspace: RightSidebarWorkspaceContext
): RightSidebarModuleAvailabilityMap {
  return Object.fromEntries(
    modules.map((module) => [
      module.id,
      resolveRightSidebarModuleAvailability(module, capabilities, workspace)
    ])
  ) as RightSidebarModuleAvailabilityMap
}

export function getRightSidebarModuleAvailability(
  availability: RightSidebarModuleAvailabilityMap,
  moduleId: RightSidebarModuleDefinition['id']
): RightSidebarModuleAvailability {
  return availability[moduleId] ?? 'available'
}

export function resolveRightSidebarPageAvailability(
  module: RightSidebarModuleDefinition,
  availability: RightSidebarModuleAvailability,
  page: RightSidebarPage
): RightSidebarModuleAvailability {
  if (
    availability === 'unavailable' &&
    module.contextBinding === 'pinned-to-creation-workspace' &&
    module.requiresWorkspace &&
    !module.requiredCapability &&
    Boolean(page.workspaceKey)
  ) {
    return 'available'
  }

  return availability
}
