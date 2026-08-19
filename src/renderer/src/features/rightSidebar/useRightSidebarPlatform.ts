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
  RightSidebarModulePageState,
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
    (moduleId: RightSidebarModuleId, moduleState?: RightSidebarModulePageState) => {
      const module = modules.find((candidate) => candidate.id === moduleId)
      if (
        !module ||
        getRightSidebarModuleAvailability(moduleAvailability, moduleId) !== 'available'
      ) {
        return
      }

      dispatch({
        module,
        moduleState,
        pageId: createPageId(moduleId),
        t,
        type: 'open',
        workspace
      })
    },
    [moduleAvailability, modules, t, workspace]
  )

  const ensureModulePage = useCallback(
    (moduleId: RightSidebarModuleId): string | null => {
      const module = modules.find((candidate) => candidate.id === moduleId)
      if (
        !module ||
        getRightSidebarModuleAvailability(moduleAvailability, moduleId) !== 'available'
      ) {
        return null
      }

      const selected = state.pages.find(
        (page) => page.id === state.activePageId && page.moduleId === moduleId
      )
      const existing = selected ?? state.pages.find((page) => page.moduleId === moduleId)
      if (existing) {
        dispatch({ pageId: existing.id, type: 'activate' })
        return existing.id
      }

      const pageId = createPageId(moduleId)
      dispatch({ module, pageId, t, type: 'open', workspace })
      return pageId
    },
    [moduleAvailability, modules, state.activePageId, state.pages, t, workspace]
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
      const targetModule = modules.find(
        (candidate) => candidate.id === (request.targetModuleId ?? module.id)
      )
      if (!targetModule) return
      dispatch({
        pageId: createPageId('page'),
        request,
        sourceModule: module,
        sourcePageId,
        t,
        targetModule,
        type: 'open-related-page'
      })
    },
    [modules, t]
  )

  const updatePage = useCallback((pageId: string, update: RightSidebarPageUpdate) => {
    dispatch({ pageId, type: 'update', update })
  }, [])

  return {
    activatePage,
    activePageId: state.activePageId,
    availableModules,
    closePage,
    ensureModulePage,
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
