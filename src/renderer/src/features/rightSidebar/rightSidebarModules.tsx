import { lazy, memo, Suspense, useCallback } from 'react'
import { Bot, FileDiff, FolderOpen, Globe2, TerminalSquare } from 'lucide-react'
import { getFileTypeIconSource } from '../../components/files/FileTypeIcon'
import type { BrowserPageMetadata } from '../browser/browserTypes'
import { browserSurfaceIdForPage } from '../browser/browserSurface'
import { useRightSidebarRuntimeContext } from './RightSidebarRuntimeContext'
import type {
  RightSidebarModuleCreateContext,
  RightSidebarModuleDefinition,
  RightSidebarModuleRenderProps,
  RightSidebarPage,
  RightSidebarPageOpenRequest
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

const FilesPanel = lazy(async () => {
  const module = await import('../files/FilesPanel')
  return { default: module.FilesPanel }
})

const AgentCenterPanel = lazy(async () => {
  const module = await import('../agentCollaboration/AgentCenterPanel')
  return { default: module.AgentCenterPanel }
})

export const MAX_FILE_PREVIEW_PAGES_PER_WORKSPACE = 20

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
    workspaceName: workspace.name,
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
    workspaceName: workspace.name,
    workspacePath: workspace.path
  }
}

function createFilesPage({
  pageId,
  t,
  workspace
}: RightSidebarModuleCreateContext): RightSidebarPage {
  return {
    id: pageId,
    moduleId: 'files',
    title: t('files.openFile'),
    workspaceKey: workspace.key,
    workspaceName: workspace.name,
    workspacePath: workspace.path
  }
}

function createAgentCenterPage({
  pageId,
  t,
  workspace
}: RightSidebarModuleCreateContext): RightSidebarPage {
  return {
    id: pageId,
    moduleId: 'agent-center',
    title: t('rightSidebar.agentCenter'),
    workspaceKey: workspace.key,
    workspaceName: workspace.name,
    workspacePath: workspace.path
  }
}

function renderTerminalModule({ activity, page, t }: RightSidebarModuleRenderProps) {
  return (
    <Suspense
      fallback={<div className="right-sidebar__panel-loading">{t('terminal.status.starting')}</div>}
    >
      <TerminalPanel initialCwd={page.workspacePath} isActive={activity === 'foreground'} />
    </Suspense>
  )
}

function renderBrowserModule({
  activity,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarModuleRenderProps) {
  return (
    <BrowserModuleSurface
      activity={activity}
      onPageUpdate={onPageUpdate}
      onSurfaceFocus={onSurfaceFocus}
      pageId={page.id}
      surfaceId={
        page.moduleState?.kind === 'browser-surface'
          ? page.moduleState.surfaceId
          : browserSurfaceIdForPage(page.id)
      }
      viewport={
        page.moduleState?.kind === 'browser-surface' ? page.moduleState.viewport : undefined
      }
      t={t}
    />
  )
}

type BrowserModuleSurfaceProps = Pick<
  RightSidebarModuleRenderProps,
  'activity' | 'onPageUpdate' | 'onSurfaceFocus' | 't'
> & {
  pageId: string
  surfaceId: string
  viewport?: { height: number; width: number }
}

const BrowserModuleSurface = memo(function BrowserModuleSurface({
  activity,
  onPageUpdate,
  onSurfaceFocus,
  pageId,
  surfaceId,
  viewport,
  t
}: BrowserModuleSurfaceProps) {
  const { browserSurfaceRequest, onBrowserSurfaceInstance, onBrowserSurfaceReady } =
    useRightSidebarRuntimeContext()
  const handlePageMetadataChange = useCallback(
    (metadata: BrowserPageMetadata) => {
      const title = metadata.title?.trim()
      onPageUpdate({
        iconUrl: metadata.iconUrl,
        ...(title ? { title } : {})
      })
    },
    [onPageUpdate]
  )

  return (
    <Suspense fallback={<div className="right-sidebar__panel-loading">{t('browser.title')}</div>}>
      <BrowserPanel
        automationRequestId={
          browserSurfaceRequest?.pageId === pageId ? browserSurfaceRequest.requestId : undefined
        }
        isActive={activity === 'foreground'}
        onAutomationSurfaceReady={(surfaceId, requestId, surfaceInstanceId, appliedViewport) =>
          onBrowserSurfaceReady?.(pageId, surfaceId, requestId, surfaceInstanceId, appliedViewport)
        }
        onPageMetadataChange={handlePageMetadataChange}
        onSurfaceInstanceChange={(surfaceId, surfaceInstanceId, isCurrent) =>
          onBrowserSurfaceInstance?.(pageId, surfaceId, surfaceInstanceId, isCurrent)
        }
        onSurfaceFocus={onSurfaceFocus}
        pageId={pageId}
        surfaceId={surfaceId}
        viewport={viewport}
      />
    </Suspense>
  )
})

function renderGitReviewModule({
  activity,
  availability,
  onOpenPage,
  page,
  t
}: RightSidebarModuleRenderProps) {
  if (availability === 'checking') {
    return <div className="right-sidebar__panel-loading">{t('gitReview.loading')}</div>
  }
  if (availability === 'unavailable') return null

  return (
    <Suspense
      fallback={<div className="right-sidebar__panel-loading">{t('gitReview.loading')}</div>}
    >
      <GitReviewModuleSurface
        isActive={activity === 'foreground'}
        onOpenFile={(path) => {
          onOpenPage(createWorkspaceFileOpenRequest(path))
        }}
        pageState={page.moduleState}
        projectId={page.workspaceKey ?? ''}
      />
    </Suspense>
  )
}

function GitReviewModuleSurface({
  isActive,
  onOpenFile,
  pageState,
  projectId
}: {
  isActive: boolean
  onOpenFile: (path: string) => void
  pageState: RightSidebarPage['moduleState']
  projectId: string
}) {
  const { activeConversationId, activeWorkspaceKey } = useRightSidebarRuntimeContext()
  const scopeNavigation =
    pageState?.kind === 'git-review' && pageState.projectId === projectId
      ? {
          filePath: pageState.filePath,
          requestId: pageState.requestId,
          scope: pageState.scope
        }
      : undefined
  return (
    <GitReviewPanel
      conversationId={projectId === activeWorkspaceKey ? activeConversationId : null}
      isActive={isActive}
      onOpenFile={onOpenFile}
      projectId={projectId}
      scopeNavigation={scopeNavigation}
    />
  )
}

function renderFilesModule({
  activity,
  availability,
  onOpenPage,
  onPageUpdate,
  onSurfaceFocus,
  page,
  t
}: RightSidebarModuleRenderProps) {
  if (availability === 'unavailable' || !page.workspaceKey) return null
  const fileState = page.moduleState?.kind === 'workspace-file' ? page.moduleState : null
  const filePath = fileState?.path ?? null
  const markdownView = fileState?.preview?.markdownView ?? 'source'
  const pdfPage = fileState?.preview?.pdfPage ?? 1
  const wrapLines = fileState?.preview?.wrapLines ?? false

  return (
    <Suspense fallback={<div className="right-sidebar__panel-loading">{t('files.loading')}</div>}>
      <FilesPanel
        filePath={filePath}
        isActive={activity === 'foreground'}
        markdownView={markdownView}
        onMarkdownViewChange={(nextMarkdownView) => {
          if (!fileState) return
          onPageUpdate({
            moduleState: {
              ...fileState,
              preview: { ...fileState?.preview, markdownView: nextMarkdownView }
            }
          })
        }}
        onOpenFile={(path) => {
          onOpenPage(createWorkspaceFileOpenRequest(path))
        }}
        onPdfPageChange={(nextPdfPage) => {
          if (!fileState) return
          onPageUpdate({
            moduleState: {
              ...fileState,
              preview: { ...fileState?.preview, pdfPage: nextPdfPage }
            }
          })
        }}
        onWrapLinesChange={(nextWrapLines) => {
          if (!fileState) return
          onPageUpdate({
            moduleState: {
              ...fileState,
              preview: { ...fileState?.preview, wrapLines: nextWrapLines }
            }
          })
        }}
        onSurfaceFocus={onSurfaceFocus}
        pdfPage={pdfPage}
        projectId={page.workspaceKey}
        projectName={page.workspaceName || t('rightSidebar.files')}
        wrapLines={wrapLines}
      />
    </Suspense>
  )
}

function renderAgentCenterModule({ onPageUpdate, page, t }: RightSidebarModuleRenderProps) {
  const pageState = page.moduleState?.kind === 'agent-center' ? page.moduleState : null
  return (
    <Suspense
      fallback={<div className="right-sidebar__panel-loading">{t('agentCenter.loading')}</div>}
    >
      <AgentCenterPanel
        onNavigate={(moduleState) => onPageUpdate({ moduleState })}
        pageState={pageState}
      />
    </Suspense>
  )
}

function createWorkspaceFileOpenRequest(path: string): RightSidebarPageOpenRequest {
  return {
    disposition: 'preview',
    iconUrl: getFileTypeIconSource(path),
    moduleState: { kind: 'workspace-file', path, tabState: 'transient' },
    resourceKey: `workspace-file:${path}`,
    targetModuleId: 'files',
    title: path.split('/').at(-1) ?? path
  }
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
    contextBinding: 'pinned-to-creation-workspace',
    createPage: createFilesPage,
    id: 'files',
    icon: FolderOpen,
    instancePolicy: 'multiple',
    maxRelatedPagesPerWorkspace: MAX_FILE_PREVIEW_PAGES_PER_WORKSPACE,
    orphanedWorkspacePolicy: 'close-page',
    render: renderFilesModule,
    requiresWorkspace: true,
    retention: 'unmount-when-inactive',
    surfaceKind: 'react',
    titleKey: 'rightSidebar.files',
    unavailablePagePolicy: 'close-page'
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

export const AGENT_CENTER_RIGHT_SIDEBAR_MODULE: RightSidebarModuleDefinition = {
  contextBinding: 'follow-workspace',
  createPage: createAgentCenterPage,
  id: 'agent-center',
  icon: Bot,
  instancePolicy: 'single',
  render: renderAgentCenterModule,
  retention: 'keep-alive',
  surfaceKind: 'react',
  titleKey: 'rightSidebar.agentCenter',
  unavailablePagePolicy: 'close-page'
}
