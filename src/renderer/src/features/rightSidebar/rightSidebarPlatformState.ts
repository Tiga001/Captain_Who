import type { Translate } from '../../config/translationFormat'
import {
  getRightSidebarModuleAvailability,
  resolveRightSidebarPageAvailability
} from './rightSidebarModuleAvailability'
import type {
  RightSidebarModuleAvailabilityMap,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModulePageState,
  RightSidebarPage,
  RightSidebarPageOpenRequest,
  RightSidebarPageUpdate,
  RightSidebarWorkspaceContext
} from './rightSidebarTypes'
import { createRightSidebarWorkspaceContext } from './rightSidebarWorkspace'

export interface RightSidebarPlatformState {
  activePageId: string | null
  pages: RightSidebarPage[]
}

export type RightSidebarPlatformAction =
  | {
      module: RightSidebarModuleDefinition
      moduleState?: RightSidebarModulePageState
      pageId: string
      t: Translate
      type: 'open'
      workspace: RightSidebarWorkspaceContext
    }
  | { pageId: string; type: 'activate' }
  | { pageId: string; type: 'close' }
  | {
      pageId: string
      request: RightSidebarPageOpenRequest
      sourceModule: RightSidebarModuleDefinition
      sourcePageId: string
      t: Translate
      targetModule: RightSidebarModuleDefinition
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
        const moduleState = action.moduleState ?? existingPage.moduleState
        if (state.activePageId === existingPage.id && moduleState === existingPage.moduleState) {
          return state
        }
        return {
          ...state,
          activePageId: existingPage.id,
          pages:
            moduleState === existingPage.moduleState
              ? state.pages
              : state.pages.map((page) =>
                  page.id === existingPage.id ? { ...page, moduleState } : page
                )
        }
      }

      const createdPage = action.module.createPage({
        existingPages: state.pages,
        pageId: action.pageId,
        t: action.t,
        workspace: action.workspace
      })
      const page = bindNewPageToWorkspace(
        action.moduleState ? { ...createdPage, moduleState: action.moduleState } : createdPage,
        action.module,
        action.workspace
      )
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
        action.sourceModule,
        action.targetModule,
        action.sourcePageId,
        action.pageId,
        action.request,
        action.t
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
  sourceModule: RightSidebarModuleDefinition,
  targetModule: RightSidebarModuleDefinition,
  sourcePageId: string,
  pageId: string,
  request: RightSidebarPageOpenRequest,
  t: Translate
): RightSidebarPlatformState {
  const sourcePage = state.pages.find((page) => page.id === sourcePageId)
  if (!sourcePage || sourcePage.moduleId !== sourceModule.id) return state

  const targetBasePage =
    targetModule.id === sourceModule.id
      ? sourcePage
      : createCrossModulePageBase(state, sourcePage, targetModule, pageId, t)
  if (!targetBasePage) return state

  const resourceKey = request.resourceKey?.trim() || undefined
  if (resourceKey) {
    const existingPage = state.pages.find(
      (page) =>
        page.moduleId === targetModule.id &&
        page.resourceKey === resourceKey &&
        page.workspaceSessionKey === targetBasePage.workspaceSessionKey
    )
    if (existingPage) {
      return state.activePageId === existingPage.id
        ? state
        : { ...state, activePageId: existingPage.id }
    }
  }

  const reusablePage =
    request.disposition === 'reuse-source-if-empty'
      ? findReusableEmptyPage(state.pages, targetModule.id, targetBasePage, sourcePage)
      : undefined
  const pageBase = reusablePage ?? targetBasePage
  const title = request.title.trim() || pageBase.title
  const page: RightSidebarPage = {
    ...pageBase,
    iconUrl: request.iconUrl === undefined ? pageBase.iconUrl : request.iconUrl,
    id: pageId,
    moduleState: request.moduleState,
    resourceKey,
    title
  }
  const pages = evictRelatedPageAtLimit(state, targetModule, targetBasePage, reusablePage?.id)
  if (reusablePage) {
    return {
      activePageId: reusablePage.id,
      pages: pages.map((currentPage) =>
        currentPage.id === reusablePage.id ? { ...page, id: reusablePage.id } : currentPage
      )
    }
  }

  return {
    activePageId: page.id,
    pages: [...pages, page]
  }
}

function createCrossModulePageBase(
  state: RightSidebarPlatformState,
  sourcePage: RightSidebarPage,
  targetModule: RightSidebarModuleDefinition,
  pageId: string,
  t: Translate
): RightSidebarPage | null {
  const workspace = createRightSidebarWorkspaceContext(
    sourcePage.workspaceKey,
    sourcePage.workspaceName,
    sourcePage.workspacePath
  )
  if (targetModule.requiresWorkspace && !workspace.hasWorkspace) return null
  return bindNewPageToWorkspace(
    targetModule.createPage({
      existingPages: state.pages,
      pageId,
      t,
      workspace
    }),
    targetModule,
    workspace
  )
}

function findReusableEmptyPage(
  pages: RightSidebarPage[],
  targetModuleId: RightSidebarModuleId,
  targetBasePage: RightSidebarPage,
  sourcePage: RightSidebarPage
): RightSidebarPage | undefined {
  const sourceIsReusable =
    sourcePage.moduleId === targetModuleId &&
    sourcePage.workspaceSessionKey === targetBasePage.workspaceSessionKey &&
    sourcePage.moduleState === undefined &&
    sourcePage.resourceKey === undefined
  if (sourceIsReusable) return sourcePage

  return pages.find(
    (page) =>
      page.moduleId === targetModuleId &&
      page.workspaceSessionKey === targetBasePage.workspaceSessionKey &&
      page.moduleState === undefined &&
      page.resourceKey === undefined
  )
}

function evictRelatedPageAtLimit(
  state: RightSidebarPlatformState,
  module: RightSidebarModuleDefinition,
  sourcePage: RightSidebarPage,
  protectedPageId: string | undefined = sourcePage.id
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
    relatedPages.find((page) => page.id !== protectedPageId && page.id !== state.activePageId) ??
    relatedPages.find((page) => page.id !== protectedPageId) ??
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
