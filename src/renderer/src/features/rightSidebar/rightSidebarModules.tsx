import { lazy, Suspense } from 'react'
import { Globe2, TerminalSquare } from 'lucide-react'
import type {
  RightSidebarModuleCreateContext,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
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

export const RIGHT_SIDEBAR_MODULES: RightSidebarModuleDefinition[] = [
  {
    createPage: createTerminalPage,
    id: 'terminal',
    icon: TerminalSquare,
    render: renderTerminalModule,
    retention: 'keep-alive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.terminal'
  },
  {
    createPage: createBrowserPage,
    id: 'browser',
    icon: Globe2,
    render: renderBrowserModule,
    retention: 'keep-alive',
    surfaceKind: 'webview',
    titleKey: 'rightSidebar.browser'
  }
]

export function getRightSidebarModule(moduleId: RightSidebarModuleId) {
  return RIGHT_SIDEBAR_MODULES.find((module) => module.id === moduleId) ?? null
}
