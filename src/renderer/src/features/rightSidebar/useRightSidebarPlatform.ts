import { useCallback, useMemo, useReducer } from 'react'
import type { Translate } from '../../config/translationFormat'
import { getRightSidebarModule } from './rightSidebarModules'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarPage,
  RightSidebarPageUpdate,
  RightSidebarWorkspaceContext
} from './rightSidebarTypes'

interface RightSidebarPlatformState {
  activePageId: string | null
  pages: RightSidebarPage[]
}

type RightSidebarPlatformAction =
  | {
      module: RightSidebarModuleDefinition
      pageId: string
      t: Translate
      type: 'open'
      workspace: RightSidebarWorkspaceContext
    }
  | { pageId: string; type: 'activate' }
  | { pageId: string; type: 'close' }
  | { pageId: string; type: 'update'; update: RightSidebarPageUpdate }

interface UseRightSidebarPlatformOptions {
  t: Translate
  workspaceKey?: string | null
  workspaceName?: string | null
  workspacePath?: string
}

const INITIAL_PLATFORM_STATE: RightSidebarPlatformState = {
  activePageId: null,
  pages: []
}

export function useRightSidebarPlatform({
  t,
  workspaceKey,
  workspaceName,
  workspacePath
}: UseRightSidebarPlatformOptions) {
  const [state, dispatch] = useReducer(reduceRightSidebarPlatform, INITIAL_PLATFORM_STATE)
  const workspace = useMemo(
    () => createWorkspaceContext(workspaceKey, workspaceName, workspacePath),
    [workspaceKey, workspaceName, workspacePath]
  )

  const openModule = useCallback(
    (moduleId: RightSidebarModuleId) => {
      const module = getRightSidebarModule(moduleId)
      if (!module) return

      dispatch({
        module,
        pageId: createPageId(moduleId),
        t,
        type: 'open',
        workspace
      })
    },
    [t, workspace]
  )

  const activatePage = useCallback((pageId: string) => {
    dispatch({ pageId, type: 'activate' })
  }, [])

  const closePage = useCallback((pageId: string) => {
    dispatch({ pageId, type: 'close' })
  }, [])

  const updatePage = useCallback((pageId: string, update: RightSidebarPageUpdate) => {
    dispatch({ pageId, type: 'update', update })
  }, [])

  return {
    activatePage,
    activePageId: state.activePageId,
    closePage,
    openModule,
    pages: state.pages,
    updatePage
  }
}

function reduceRightSidebarPlatform(
  state: RightSidebarPlatformState,
  action: RightSidebarPlatformAction
): RightSidebarPlatformState {
  switch (action.type) {
    case 'open': {
      const page = action.module.createPage({
        existingPages: state.pages,
        pageId: action.pageId,
        t: action.t,
        workspace: action.workspace
      })
      return {
        activePageId: page.id,
        pages: [...state.pages, page]
      }
    }
    case 'activate':
      return state.pages.some((page) => page.id === action.pageId)
        ? { ...state, activePageId: action.pageId }
        : state
    case 'close': {
      const pageIndex = state.pages.findIndex((page) => page.id === action.pageId)
      if (pageIndex < 0) return state

      const pages = state.pages.filter((page) => page.id !== action.pageId)
      if (state.activePageId !== action.pageId) {
        return {
          activePageId: pages.some((page) => page.id === state.activePageId)
            ? state.activePageId
            : (pages[0]?.id ?? null),
          pages
        }
      }

      return {
        activePageId: pages[Math.min(pageIndex, pages.length - 1)]?.id ?? null,
        pages
      }
    }
    case 'update': {
      let changed = false
      const pages = state.pages.map((page) => {
        if (page.id !== action.pageId) return page

        const title = action.update.title?.trim() || page.title
        const iconUrl = action.update.iconUrl === undefined ? page.iconUrl : action.update.iconUrl
        if (title === page.title && iconUrl === page.iconUrl) return page

        changed = true
        return { ...page, iconUrl, title }
      })
      return changed ? { ...state, pages } : state
    }
  }
}

function createWorkspaceContext(
  workspaceKey: string | null | undefined,
  workspaceName: string | null | undefined,
  workspacePath: string | undefined
): RightSidebarWorkspaceContext {
  const pathName = workspacePath?.split(/[\\/]/).filter(Boolean).at(-1)?.trim()
  const name = workspaceName?.trim() || pathName || null

  return {
    key: workspaceKey || workspacePath || workspaceName || 'home',
    name,
    path: workspacePath
  }
}

function createPageId(moduleId: RightSidebarModuleId): string {
  return `${moduleId}-${crypto.randomUUID()}`
}
