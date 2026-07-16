import { lazy, Suspense } from 'react'
import { FileDiff, Globe2, TerminalSquare } from 'lucide-react'
import type {
  RightSidebarModuleCreateContext,
  RightSidebarModuleDefinition,
  RightSidebarModuleRenderProps,
  RightSidebarPage
} from './rightSidebarTypes'

const TerminalPanel = lazy(async () => {
  const module = await import('../terminal/TerminalPanel')
  return { default: module.TerminalPanel }
})

const BrowserPanel = lazy(async () => {
  const module = await import('../browser/BrowserPanel')
  return { default: module.BrowserPanel }
})

const GitReviewPanel = lazy(async () => {
  const module = await import('../gitReview/GitReviewPanel')
  return { default: module.GitReviewPanel }
})

function createTerminalPage({
  existingPages,
  pageId,
  t,
  workspace
}: RightSidebarModuleCreateContext): RightSidebarPage {
  const existingCount = existingPages.filter(
    (page) => page.moduleId === 'terminal' && page.workspaceKey === workspace.key
  ).length
  const baseTitle = workspace.name || t('terminal.title')

  return {
    id: pageId,
    moduleId: 'terminal',
    title: existingCount === 0 ? baseTitle : `${baseTitle} (${existingCount})`,
    workspaceKey: workspace.key,
    workspacePath: workspace.path
  }
}

function createBrowserPage({
  existingPages,
  pageId,
  t
}: RightSidebarModuleCreateContext): RightSidebarPage {
  const existingCount = existingPages.filter((page) => page.moduleId === 'browser').length
  const baseTitle = t('browser.newTab')

  return {
    iconUrl: null,
    id: pageId,
    moduleId: 'browser',
    title: existingCount === 0 ? baseTitle : `${baseTitle} (${existingCount})`,
    workspaceKey: null
  }
}

function createGitReviewPage({
  pageId,
  t,
  workspace
}: RightSidebarModuleCreateContext): RightSidebarPage {
  return {
    id: pageId,
    moduleId: 'git-review',
    title: t('rightSidebar.review'),
    workspaceKey: workspace.key,
    workspacePath: workspace.path
  }
}

function renderTerminalModule({ isActive, page, t }: RightSidebarModuleRenderProps) {
  return (
    <Suspense
      fallback={<div className="right-sidebar__panel-loading">{t('terminal.status.starting')}</div>}
    >
      <TerminalPanel initialCwd={page.workspacePath} isActive={isActive} />
    </Suspense>
  )
}

function renderBrowserModule({
  isActive,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarModuleRenderProps) {
  return (
    <Suspense fallback={<div className="right-sidebar__panel-loading">{t('browser.title')}</div>}>
      <BrowserPanel
        isActive={isActive}
        onPageMetadataChange={(metadata) => {
          onPageUpdate({
            iconUrl: metadata.iconUrl,
            title: metadata.title?.trim() || page.title
          })
        }}
        onSurfaceFocus={onSurfaceFocus}
        pageId={page.id}
      />
    </Suspense>
  )
}

function renderGitReviewModule({ availability, isActive, page, t }: RightSidebarModuleRenderProps) {
  if (availability === 'checking') {
    return <div className="right-sidebar__panel-loading">{t('gitReview.loading')}</div>
  }
  if (availability === 'unavailable') return null

  return (
    <Suspense
      fallback={<div className="right-sidebar__panel-loading">{t('gitReview.loading')}</div>}
    >
      <GitReviewPanel isActive={isActive} projectId={page.workspaceKey ?? ''} />
    </Suspense>
  )
}

export const RIGHT_SIDEBAR_MODULES: RightSidebarModuleDefinition[] = [
  {
    contextBinding: 'pinned-to-creation-workspace',
    createPage: createTerminalPage,
    id: 'terminal',
    icon: TerminalSquare,
    instancePolicy: 'multiple',
    render: renderTerminalModule,
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.terminal',
    unavailablePagePolicy: 'retain-page'
  },
  {
    contextBinding: 'global',
    createPage: createBrowserPage,
    id: 'browser',
    icon: Globe2,
    instancePolicy: 'multiple',
    render: renderBrowserModule,
    retention: 'keep-alive',
    surfaceKind: 'webview',
    titleKey: 'rightSidebar.browser',
    unavailablePagePolicy: 'retain-page'
  },
  {
    contextBinding: 'follow-workspace',
    createPage: createGitReviewPage,
    id: 'git-review',
    icon: FileDiff,
    instancePolicy: 'single',
    render: renderGitReviewModule,
    requiredCapability: 'git-repository',
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.review',
    unavailablePagePolicy: 'close-page'
  }
]
