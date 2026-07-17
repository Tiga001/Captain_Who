import type { Translate } from '../../config/translationFormat'
import {
  getRightSidebarModuleAvailability,
  resolveRightSidebarPageAvailability
} from './rightSidebarModuleAvailability'
import type {
  RightSidebarModuleAvailabilityMap,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarPage,
  RightSidebarPageOpenRequest,
  RightSidebarPageUpdate,
  RightSidebarWorkspaceContext
} from './rightSidebarTypes'

export interface RightSidebarPlatformState {
  activePageId: string | null
  pages: RightSidebarPage[]
}

export type RightSidebarPlatformAction =
  | {
      module: RightSidebarModuleDefinition
      pageId: string
      t: Translate
      type: 'open'
      workspace: RightSidebarWorkspaceContext
    }
  | { pageId: string; type: 'activate' }
  | { pageId: string; type: 'close' }
  | {
      module: RightSidebarModuleDefinition
      pageId: string
      request: RightSidebarPageOpenRequest
      sourcePageId: string
      type: 'open-related-page'
    }
  | {
      availability: RightSidebarModuleAvailabilityMap
      modules: RightSidebarModuleDefinition[]
      type: 'synchronize-context'
      workspace: RightSidebarWorkspaceContext
      workspaceKeys?: readonly string[]
    }
  | { pageId: string; type: 'update'; update: RightSidebarPageUpdate }

export const INITIAL_RIGHT_SIDEBAR_PLATFORM_STATE: RightSidebarPlatformState = {
  activePageId: null,
  pages: []
}

export function reduceRightSidebarPlatform(
  state: RightSidebarPlatformState,
  action: RightSidebarPlatformAction
): RightSidebarPlatformState {
  switch (action.type) {
    case 'open': {
      const existingPage = findExistingPage(state.pages, action.module, action.workspace)
      if (existingPage) {
        return state.activePageId === existingPage.id
          ? state
          : { ...state, activePageId: existingPage.id }
      }

      const createdPage = action.module.createPage({
        existingPages: state.pages,
        pageId: action.pageId,
        t: action.t,
        workspace: action.workspace
      })
      const page = bindNewPageToWorkspace(createdPage, action.module, action.workspace)
      return {
        activePageId: page.id,
        pages: [...state.pages, page]
      }
    }
    case 'activate':
      return state.pages.some((page) => page.id === action.pageId) &&
        state.activePageId !== action.pageId
        ? { ...state, activePageId: action.pageId }
        : state
    case 'close':
      return closePage(state, action.pageId)
    case 'open-related-page':
      return openRelatedPage(
        state,
        action.module,
        action.sourcePageId,
        action.pageId,
        action.request
      )
    case 'synchronize-context':
      return synchronizeContext(
        state,
        action.modules,
        action.availability,
        action.workspace,
        action.workspaceKeys
      )
    case 'update': {
      let changed = false
      const pages = state.pages.map((page) => {
        if (page.id !== action.pageId) return page

        const title = action.update.title?.trim() || page.title
        const iconUrl = action.update.iconUrl === undefined ? page.iconUrl : action.update.iconUrl
        const moduleState =
          action.update.moduleState === undefined ? page.moduleState : action.update.moduleState
        if (title === page.title && iconUrl === page.iconUrl && moduleState === page.moduleState) {
          return page
        }

        changed = true
        return { ...page, iconUrl, moduleState, title }
      })
      return changed ? { ...state, pages } : state
    }
  }
}

function openRelatedPage(
  state: RightSidebarPlatformState,
  module: RightSidebarModuleDefinition,
  sourcePageId: string,
  pageId: string,
  request: RightSidebarPageOpenRequest
): RightSidebarPlatformState {
  const sourcePage = state.pages.find((page) => page.id === sourcePageId)
  if (!sourcePage || sourcePage.moduleId !== module.id) return state

  const resourceKey = request.resourceKey?.trim() || undefined
  if (resourceKey) {
    const existingPage = state.pages.find(
      (page) =>
        page.moduleId === sourcePage.moduleId &&
        page.resourceKey === resourceKey &&
        page.workspaceSessionKey === sourcePage.workspaceSessionKey
    )
    if (existingPage) {
      return state.activePageId === existingPage.id
        ? state
        : { ...state, activePageId: existingPage.id }
    }
  }

  const title = request.title.trim() || sourcePage.title
  const page: RightSidebarPage = {
    ...sourcePage,
    iconUrl: request.iconUrl === undefined ? sourcePage.iconUrl : request.iconUrl,
    id: pageId,
    moduleState: request.moduleState,
    resourceKey,
    title
  }
  const pages = evictRelatedPageAtLimit(state, module, sourcePage)
  if (
    request.disposition === 'reuse-source-if-empty' &&
    sourcePage.moduleState === undefined &&
    sourcePage.resourceKey === undefined
  ) {
    return {
      activePageId: sourcePage.id,
      pages: pages.map((currentPage) =>
        currentPage.id === sourcePage.id ? { ...page, id: sourcePage.id } : currentPage
      )
    }
  }

  return {
    activePageId: page.id,
    pages: [...pages, page]
  }
}

function evictRelatedPageAtLimit(
  state: RightSidebarPlatformState,
  module: RightSidebarModuleDefinition,
  sourcePage: RightSidebarPage
): RightSidebarPage[] {
  const limit = module.maxRelatedPagesPerWorkspace
  if (!limit || limit < 1) return state.pages

  const relatedPages = state.pages.filter(
    (page) =>
      page.moduleId === module.id &&
      page.workspaceSessionKey === sourcePage.workspaceSessionKey &&
      Boolean(page.resourceKey)
  )
  if (relatedPages.length < limit) return state.pages

  const pageToEvict =
    relatedPages.find((page) => page.id !== sourcePage.id && page.id !== state.activePageId) ??
    relatedPages.find((page) => page.id !== sourcePage.id) ??
    relatedPages[0]
  if (!pageToEvict) return state.pages
  return state.pages.filter((page) => page.id !== pageToEvict.id)
}

function findExistingPage(
  pages: RightSidebarPage[],
  module: RightSidebarModuleDefinition,
  workspace: RightSidebarWorkspaceContext
): RightSidebarPage | undefined {
  if (module.instancePolicy === 'multiple') return undefined
  if (module.instancePolicy === 'single') {
    return pages.find((page) => page.moduleId === module.id)
  }
  return pages.find(
    (page) => page.moduleId === module.id && page.workspaceSessionKey === workspace.sessionKey
  )
}

function bindNewPageToWorkspace(
  page: RightSidebarPage,
  module: RightSidebarModuleDefinition,
  workspace: RightSidebarWorkspaceContext
): RightSidebarPage {
  if (module.contextBinding === 'global') {
    return page.workspaceSessionKey === null ? page : { ...page, workspaceSessionKey: null }
  }
  if (module.contextBinding === 'pinned-to-creation-workspace') {
    return page.workspaceSessionKey === workspace.sessionKey
      ? page
      : { ...page, workspaceSessionKey: workspace.sessionKey }
  }
  return rebindPageToWorkspace(page, workspace)
}

function rebindPageToWorkspace(
  page: RightSidebarPage,
  workspace: RightSidebarWorkspaceContext
): RightSidebarPage {
  if (
    page.workspaceKey === workspace.key &&
    page.workspaceName === workspace.name &&
    page.workspacePath === workspace.path &&
    page.workspaceSessionKey === workspace.sessionKey
  ) {
    return page
  }
  return {
    ...page,
    workspaceKey: workspace.key,
    workspaceName: workspace.name,
    workspacePath: workspace.path,
    workspaceSessionKey: workspace.sessionKey
  }
}

function synchronizeContext(
  state: RightSidebarPlatformState,
  modules: RightSidebarModuleDefinition[],
  availability: RightSidebarModuleAvailabilityMap,
  workspace: RightSidebarWorkspaceContext,
  workspaceKeys?: readonly string[]
): RightSidebarPlatformState {
  const modulesById = new Map(modules.map((module) => [module.id, module]))
  const knownWorkspaceKeys = workspaceKeys ? new Set(workspaceKeys) : null
  const seenSingleModules = new Set<RightSidebarModuleId>()
  let changed = false
  const pages: RightSidebarPage[] = []

  for (const currentPage of state.pages) {
    const module = modulesById.get(currentPage.moduleId)
    if (!module) {
      changed = true
      continue
    }

    if (
      knownWorkspaceKeys &&
      module.orphanedWorkspacePolicy === 'close-page' &&
      currentPage.workspaceKey &&
      !knownWorkspaceKeys.has(currentPage.workspaceKey)
    ) {
      changed = true
      continue
    }

    const moduleAvailability = resolveRightSidebarPageAvailability(
      module,
      getRightSidebarModuleAvailability(availability, module.id),
      currentPage
    )
    if (moduleAvailability === 'unavailable' && module.unavailablePagePolicy === 'close-page') {
      changed = true
      continue
    }

    if (module.instancePolicy === 'single') {
      if (seenSingleModules.has(module.id)) {
        changed = true
        continue
      }
      seenSingleModules.add(module.id)
    }

    const page =
      module.contextBinding === 'follow-workspace'
        ? rebindPageToWorkspace(currentPage, workspace)
        : currentPage
    changed ||= page !== currentPage
    pages.push(page)
  }

  if (!changed) return state
  return {
    activePageId: resolveActivePageIdAfterReconcile(state, pages),
    pages
  }
}

function closePage(state: RightSidebarPlatformState, pageId: string): RightSidebarPlatformState {
  const pageIndex = state.pages.findIndex((page) => page.id === pageId)
  if (pageIndex < 0) return state

  const pages = state.pages.filter((page) => page.id !== pageId)
  if (state.activePageId !== pageId) {
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

function resolveActivePageIdAfterReconcile(
  state: RightSidebarPlatformState,
  pages: RightSidebarPage[]
): string | null {
  if (pages.some((page) => page.id === state.activePageId)) return state.activePageId
  if (pages.length === 0) return null

  const activeIndex = state.pages.findIndex((page) => page.id === state.activePageId)
  if (activeIndex < 0) return pages[0]?.id ?? null

  const remainingIds = new Set(pages.map((page) => page.id))
  for (let index = activeIndex + 1; index < state.pages.length; index += 1) {
    const candidate = state.pages[index]
    if (candidate && remainingIds.has(candidate.id)) return candidate.id
  }
  for (let index = activeIndex - 1; index >= 0; index -= 1) {
    const candidate = state.pages[index]
    if (candidate && remainingIds.has(candidate.id)) return candidate.id
  }
  return pages[0]?.id ?? null
}
