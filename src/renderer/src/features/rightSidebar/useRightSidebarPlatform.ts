import { useCallback, useLayoutEffect, useMemo, useReducer } from 'react'
import type { Translate } from '../../config/translationFormat'
import {
  getRightSidebarModuleAvailability,
  resolveRightSidebarModuleAvailabilityMap
} from './rightSidebarModuleAvailability'
import {
  INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE,
  reduceRightSidebarPlatform
} from './rightSidebarPlatformState'
import type {
  RightSidebarCapabilities,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarPageOpenRequest,
  RightSidebarPageUpdate
} from './rightSidebarTypes'
import { createRightSidebarWorkspaceContext } from './rightSidebarWorkspace'

interface UseRightSidebarPlatformOptions {
  capabilities?: RightSidebarCapabilities
  t: Translate
  workspaceKey?: string | null
  workspaceKeys?: readonly string[]
  workspaceName?: string | null
  workspacePath?: string
  modules: RightSidebarModuleDefinition[]
}

const EMPTY_CAPABILITIES: RightSidebarCapabilities = {}

export function useRightSidebarPlatform({
  capabilities = EMPTY_CAPABILITIES,
  modules,
  t,
  workspaceKey,
  workspaceKeys,
  workspaceName,
  workspacePath
}: UseRightSidebarPlatformOptions) {
  const [state, dispatch] = useReducer(
    reduceRightSidebarPlatform,
    INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE
  )
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

  const openModule = useCallback(
    (moduleId: RightSidebarModuleId) => {
      const module = modules.find((candidate) => candidate.id === moduleId)
      if (
        !module ||
        getRightSidebarModuleAvailability(moduleAvailability, moduleId) !== 'available'
      ) {
        return
      }

      dispatch({
        module,
        pageId: createPageId(moduleId),
        t,
        type: 'open',
        workspace
      })
    },
    [moduleAvailability, modules, t, workspace]
  )

  useLayoutEffect(() => {
    dispatch({
      availability: moduleAvailability,
      modules,
      type: 'synchronize-context',
      workspace,
      workspaceKeys
    })
  }, [moduleAvailability, modules, workspace, workspaceKeys])

  const activatePage = useCallback((pageId: string) => {
    dispatch({ pageId, type: 'activate' })
  }, [])

  const closePage = useCallback((pageId: string) => {
    dispatch({ pageId, type: 'close' })
  }, [])

  const openRelatedPage = useCallback(
    (
      sourcePageId: string,
      module: RightSidebarModuleDefinition,
      request: RightSidebarPageOpenRequest
    ) => {
      dispatch({
        module,
        pageId: createPageId('page'),
        request,
        sourcePageId,
        type: 'open-related-page'
      })
    },
    []
  )

  const updatePage = useCallback((pageId: string, update: RightSidebarPageUpdate) => {
    dispatch({ pageId, type: 'update', update })
  }, [])

  return {
    activatePage,
    activePageId: state.activePageId,
    availableModules,
    closePage,
    moduleAvailability,
    openModule,
    openRelatedPage,
    pages: state.pages,
    updatePage
  }
}

function createPageId(moduleId: RightSidebarModuleId | 'page'): string {
  return `${moduleId}-${crypto.randomUUID()}`
}
