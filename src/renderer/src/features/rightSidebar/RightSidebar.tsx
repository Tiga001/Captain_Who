import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { Maximize } from 'lucide-react'
import type {
  BrowserSurfaceCommand,
  BrowserSurfaceReadyInput,
  BrowserSurfaceReadyOutput
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { WorkspaceFileTreeSessionsProvider } from '../files/WorkspaceFileTreeSessions'
import { RightSidebarHome } from './RightSidebarHome'
import { RightSidebarTabStrip } from './RightSidebarTabStrip'
import { RightSidebarPageStack } from './RightSidebarPageStack'
import { RightSidebarRuntimeContext } from './RightSidebarRuntimeContext'
import { useRightSidebarDocumentVisibility } from './rightSidebarActivity'
import { RIGHT_SIDEBAR_MODULES } from './rightSidebarModules'
import { useRightSidebarModules } from './useRightSidebarModules'
import { createRightSidebarWorkspaceSessionKey } from './rightSidebarWorkspace'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import type {
  RightSidebarCapabilities,
  AgentObserverRenderContext,
  RightSidebarAgentNavigationRequest,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarModuleNavigationRequest,
  RightSidebarReviewNavigationRequest
} from './rightSidebarTypes'
import { useRightSidebarPlatform } from './useRightSidebarPlatform'
import {
  browserSurfaceIdForPage,
  clearBrowserSurfaceSelection,
  resolveBrowserSurfaceHostApi
} from '../browser/browserSurface'
import './RightSidebar.css'

interface RightSidebarProps {
  activeConversationId?: string | null
  agentNavigationRequest?: RightSidebarAgentNavigationRequest | null
  browserSurfaceCommand?: BrowserSurfaceCommand | null
  capabilities?: RightSidebarCapabilities
  collaborationSnapshot?: CollaborationStoreSnapshot | null
  isMaximized: boolean
  isOpen: boolean
  isWorkspaceVisible?: boolean
  maximizedToolbarControls?: ReactNode
  modules?: RightSidebarModuleDefinition[]
  moduleNavigationRequest?: RightSidebarModuleNavigationRequest | null
  onBrowserSurfaceReady?: (input: BrowserSurfaceReadyInput) => Promise<BrowserSurfaceReadyOutput>
  onOpenAgentTemplates?: () => void
  onOpenBrowserSettings?: (destination: 'settings' | 'downloads' | 'history') => void
  onToggleMaximized: () => void
  reviewNavigationRequest?: RightSidebarReviewNavigationRequest | null
  renderAgentObserver?: (context: AgentObserverRenderContext) => ReactNode
  workspaceKey?: string | null
  workspaceKeys?: readonly string[]
  projectWorkspaceRevisions?: Readonly<Record<string, string>>
  workspaceName?: string | null
  workspacePath?: string
}

const MAX_SURFACE_READY_ATTEMPTS = 32

async function submitBrowserSurfaceReadyUntilSettled(input: {
  input: BrowserSurfaceReadyInput
  isCurrent: () => boolean
  submit: (value: BrowserSurfaceReadyInput) => Promise<BrowserSurfaceReadyOutput>
}): Promise<void> {
  for (let attempt = 0; attempt < MAX_SURFACE_READY_ATTEMPTS && input.isCurrent(); attempt += 1) {
    const output = await input.submit(input.input)
    if (!output.retryable) return
    const delayMs = Math.min(500, 20 * 2 ** attempt)
    await new Promise<void>((resolve) => setTimeout(resolve, delayMs))
  }
}

function RestoreFromMaximizedIcon(): ReactNode {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M10 4v6H4" />
      <path d="M14 4v6h6" />
      <path d="M10 20v-6H4" />
      <path d="M14 20v-6h6" />
    </svg>
  )
}

export const RightSidebar = memo(function RightSidebar({
  activeConversationId,
  agentNavigationRequest,
  browserSurfaceCommand,
  capabilities,
  collaborationSnapshot = null,
  isMaximized,
  isOpen,
  isWorkspaceVisible = true,
  maximizedToolbarControls,
  modules: configuredModules = RIGHT_SIDEBAR_MODULES,
  moduleNavigationRequest,
  onBrowserSurfaceReady,
  onOpenAgentTemplates,
  onOpenBrowserSettings,
  onToggleMaximized,
  reviewNavigationRequest,
  renderAgentObserver,
  workspaceKey,
  workspaceKeys,
  projectWorkspaceRevisions,
  workspaceName,
  workspacePath
}: RightSidebarProps): ReactNode {
  const { t } = useFrontendConfig()
  const documentVisible = useRightSidebarDocumentVisibility()
  const handledReviewNavigationRequestIdRef = useRef<number | null>(null)
  const handledAgentNavigationRequestIdRef = useRef<number | null>(null)
  const handledBrowserSurfaceRequestIdRef = useRef<string | null>(null)
  const handledModuleNavigationRequestIdRef = useRef<number | null>(null)
  const submittedBrowserSurfaceRequestRef = useRef<{
    requestId: string
    surfaceInstanceId: string
  } | null>(null)
  const [isModuleMenuOpen, setIsModuleMenuOpen] = useState(false)
  const [browserSurfaceRequest, setBrowserSurfaceRequest] = useState<{
    pageId: string
    requestId: string
  } | null>(null)
  const browserSurfaceRequestRef = useRef(browserSurfaceRequest)
  const [browserSurfaceInstances, setBrowserSurfaceInstances] = useState<
    ReadonlyMap<string, string>
  >(() => new Map())
  const browserSurfaceInstancesRef = useRef(browserSurfaceInstances)
  const { childAgents, modules } = useRightSidebarModules({
    activeConversationId,
    collaborationSnapshot,
    configuredModules
  })
  const {
    activatePage,
    activePageId,
    availableModules,
    closePage,
    moduleAvailability,
    openModule: openPlatformModule,
    openModulePage,
    openRelatedPage,
    pages,
    updatePage
  } = useRightSidebarPlatform({
    capabilities,
    modules,
    t,
    workspaceKey,
    workspaceKeys,
    workspaceName,
    workspacePath
  })
  const [agentBrowserSurfaces, setAgentBrowserSurfaces] = useState<ReadonlyMap<string, string>>(
    () => new Map()
  )
  const handleBrowserAutomationTargetChange = useCallback(
    (surfaceId: string, instanceId: string, isTarget: boolean) => {
      if (browserSurfaceInstancesRef.current.get(surfaceId) !== instanceId) return
      setAgentBrowserSurfaces((current) => {
        if (isTarget ? current.get(surfaceId) === instanceId : !current.has(surfaceId))
          return current
        const next = new Map(current)
        if (isTarget) next.set(surfaceId, instanceId)
        else next.delete(surfaceId)
        return next
      })
    },
    []
  )

  const automationPageIds = useMemo(
    () =>
      new Set(
        pages
          .filter((page) => {
            if (page.moduleId !== 'browser') return false
            const surfaceId =
              page.moduleState?.kind === 'browser-surface'
                ? page.moduleState.surfaceId
                : browserSurfaceIdForPage(page.id)
            const instanceId = agentBrowserSurfaces.get(surfaceId)
            return instanceId !== undefined && browserSurfaceInstances.get(surfaceId) === instanceId
          })
          .map((page) => page.id)
      ),
    [agentBrowserSurfaces, browserSurfaceInstances, pages]
  )

  useEffect(() => {
    if (
      !browserSurfaceCommand ||
      handledBrowserSurfaceRequestIdRef.current === browserSurfaceCommand.requestId
    ) {
      return
    }
    const surfaceForPage = (page: (typeof pages)[number]): string | null =>
      page.moduleId === 'browser'
        ? page.moduleState?.kind === 'browser-surface'
          ? page.moduleState.surfaceId
          : browserSurfaceIdForPage(page.id)
        : null

    if (browserSurfaceCommand.kind === 'closeSurface') {
      const page = pages.find(
        (candidate) => surfaceForPage(candidate) === browserSurfaceCommand.surfaceId
      )
      const expectedInstanceId = browserSurfaceCommand.surfaceInstanceId
      const currentInstanceId = browserSurfaceInstances.get(browserSurfaceCommand.surfaceId)
      if (expectedInstanceId === undefined) {
        // Parsing stays backward compatible, but a generationless close is never authoritative.
        handledBrowserSurfaceRequestIdRef.current = browserSurfaceCommand.requestId
        return
      }
      if (currentInstanceId === undefined) return

      handledBrowserSurfaceRequestIdRef.current = browserSurfaceCommand.requestId
      if (currentInstanceId !== expectedInstanceId) return
      if (page) closePage(page.id)
      setBrowserSurfaceInstances((current) => {
        if (current.get(browserSurfaceCommand.surfaceId) !== expectedInstanceId) return current
        const next = new Map(current)
        next.delete(browserSurfaceCommand.surfaceId)
        browserSurfaceInstancesRef.current = next
        return next
      })
      setAgentBrowserSurfaces((current) => {
        const next = new Map(current)
        next.delete(browserSurfaceCommand.surfaceId)
        return next
      })
      setBrowserSurfaceRequest((current) => {
        const next = current?.pageId === page?.id ? null : current
        browserSurfaceRequestRef.current = next
        return next
      })
      return
    }

    handledBrowserSurfaceRequestIdRef.current = browserSurfaceCommand.requestId

    const existing = pages.find(
      (candidate) => surfaceForPage(candidate) === browserSurfaceCommand.surfaceId
    )
    if (browserSurfaceCommand.kind === 'resizeSurface' && existing) {
      updatePage(existing.id, {
        moduleState: {
          ...(existing.moduleState?.kind === 'browser-surface' ? existing.moduleState : {}),
          kind: 'browser-surface',
          surfaceId: browserSurfaceCommand.surfaceId,
          viewport: {
            height: browserSurfaceCommand.height,
            width: browserSurfaceCommand.width
          }
        }
      })
    }
    const pageId = existing
      ? existing.id
      : browserSurfaceCommand.kind === 'selectSurface'
        ? null
        : openModulePage(
            'browser',
            {
              kind: 'browser-surface',
              surfaceId: browserSurfaceCommand.surfaceId
            },
            browserSurfaceCommand.kind !== 'createSurface' || browserSurfaceCommand.activate
          )
    if (!pageId) return
    const shouldActivate =
      browserSurfaceCommand.kind !== 'createSurface' || browserSurfaceCommand.activate
    if (shouldActivate) {
      activatePage(pageId)
    }
    submittedBrowserSurfaceRequestRef.current = null
    const nextRequest = { pageId, requestId: browserSurfaceCommand.requestId }
    browserSurfaceRequestRef.current = nextRequest
    setBrowserSurfaceRequest(nextRequest)
  }, [
    activatePage,
    browserSurfaceCommand,
    browserSurfaceInstances,
    closePage,
    openModulePage,
    pages,
    updatePage
  ])

  const handleBrowserSurfaceInstance = useCallback(
    (pageId: string, surfaceId: string, surfaceInstanceId: string, isCurrent: boolean): void => {
      const page = pages.find((candidate) => candidate.id === pageId)
      const expectedSurfaceId =
        page?.moduleState?.kind === 'browser-surface'
          ? page.moduleState.surfaceId
          : browserSurfaceIdForPage(pageId)
      if (expectedSurfaceId !== surfaceId) return

      const current = browserSurfaceInstancesRef.current
      if (isCurrent && current.get(surfaceId) === surfaceInstanceId) return
      if (!isCurrent && current.get(surfaceId) !== surfaceInstanceId) return
      const next = new Map(current)
      if (isCurrent) next.set(surfaceId, surfaceInstanceId)
      else next.delete(surfaceId)
      browserSurfaceInstancesRef.current = next
      setBrowserSurfaceInstances(next)
    },
    [pages]
  )

  const handleBrowserSurfaceReady = useCallback(
    (
      pageId: string,
      surfaceId: string,
      requestId: string,
      surfaceInstanceId: string,
      viewport?: { height: number; width: number }
    ): void => {
      const page = pages.find((candidate) => candidate.id === pageId)
      const expectedSurfaceId =
        page?.moduleState?.kind === 'browser-surface'
          ? page.moduleState.surfaceId
          : browserSurfaceIdForPage(pageId)
      if (
        browserSurfaceRequest?.pageId !== pageId ||
        browserSurfaceRequest.requestId !== requestId ||
        expectedSurfaceId !== surfaceId ||
        browserSurfaceInstancesRef.current.get(surfaceId) !== surfaceInstanceId ||
        (submittedBrowserSurfaceRequestRef.current?.requestId === requestId &&
          submittedBrowserSurfaceRequestRef.current.surfaceInstanceId === surfaceInstanceId)
      ) {
        return
      }
      submittedBrowserSurfaceRequestRef.current = { requestId, surfaceInstanceId }
      if (!onBrowserSurfaceReady) return
      const input: BrowserSurfaceReadyInput = {
        schemaVersion: 1,
        requestId,
        surfaceId,
        surfaceInstanceId,
        ...(viewport ? { viewport } : {})
      }
      void submitBrowserSurfaceReadyUntilSettled({
        input,
        isCurrent: () =>
          browserSurfaceRequestRef.current?.pageId === pageId &&
          browserSurfaceRequestRef.current.requestId === requestId &&
          browserSurfaceInstancesRef.current.get(surfaceId) === surfaceInstanceId,
        submit: onBrowserSurfaceReady
      }).finally(() => {
        const submitted = submittedBrowserSurfaceRequestRef.current
        if (
          submitted?.requestId === requestId &&
          submitted.surfaceInstanceId === surfaceInstanceId
        ) {
          submittedBrowserSurfaceRequestRef.current = null
        }
        if (
          browserSurfaceRequestRef.current?.requestId !== requestId ||
          browserSurfaceInstancesRef.current.get(surfaceId) !== surfaceInstanceId
        ) {
          return
        }
        browserSurfaceRequestRef.current = null
        setBrowserSurfaceRequest((current) => (current?.requestId === requestId ? null : current))
      })
    },
    [browserSurfaceRequest, onBrowserSurfaceReady, pages]
  )
  const hasOpenPages = pages.length > 0
  const fileTreeProjectIds = useMemo(
    () => [
      ...new Set(
        pages.flatMap((page) =>
          page.moduleId === 'files' && page.workspaceKey ? [page.workspaceKey] : []
        )
      )
    ],
    [pages]
  )
  // `data-right-open=false` is the layout's final visibility authority, including while the
  // maximize preference remains set for a later reopen.
  const sidebarVisible = isOpen && isWorkspaceVisible
  const hasForegroundBrowser =
    sidebarVisible &&
    documentVisible &&
    pages.some((page) => page.id === activePageId && page.moduleId === 'browser')
  const maximizeLabel = isMaximized ? t('rightSidebar.restore') : t('rightSidebar.maximize')
  const runtimeContext = useMemo(
    () => ({
      activeConversationId: activeConversationId ?? null,
      activeWorkspaceKey: workspaceKey ?? null,
      collaborationSnapshot,
      browserSurfaceRequest: browserSurfaceRequest ?? undefined,
      onBrowserSurfaceInstance: handleBrowserSurfaceInstance,
      onBrowserAutomationTargetChange: handleBrowserAutomationTargetChange,
      onBrowserSurfaceReady: handleBrowserSurfaceReady,
      onOpenAgentTemplates,
      onOpenBrowserSettings,
      renderAgentObserver
    }),
    [
      activeConversationId,
      browserSurfaceRequest,
      collaborationSnapshot,
      handleBrowserSurfaceInstance,
      handleBrowserAutomationTargetChange,
      handleBrowserSurfaceReady,
      onOpenAgentTemplates,
      onOpenBrowserSettings,
      renderAgentObserver,
      workspaceKey
    ]
  )

  const closeTransientUi = useCallback(() => {
    setIsModuleMenuOpen(false)
  }, [])
  const clearCurrentBrowserSelection = useCallback((): void => {
    const browser = resolveBrowserSurfaceHostApi()
    if (!browser) return
    void clearBrowserSurfaceSelection(browser).catch((error: unknown) => {
      console.error('Failed to clear the active Browser surface', error)
    })
  }, [])

  useEffect(() => {
    if (!sidebarVisible) closeTransientUi()
  }, [closeTransientUi, sidebarVisible])

  useEffect(() => {
    if (hasForegroundBrowser) return
    clearCurrentBrowserSelection()
  }, [clearCurrentBrowserSelection, hasForegroundBrowser])

  const openModule = useCallback(
    (moduleId: RightSidebarModuleId) => {
      if (moduleId === 'agent-center') {
        if (!activeConversationId || childAgents.length === 0) return
        openPlatformModule(moduleId, {
          kind: 'agent-center',
          rootConversationId: activeConversationId,
          view: 'list'
        })
      } else {
        openPlatformModule(moduleId)
      }
      closeTransientUi()
    },
    [activeConversationId, childAgents.length, closeTransientUi, openPlatformModule]
  )

  useEffect(() => {
    const request = moduleNavigationRequest
    if (
      !request ||
      (handledModuleNavigationRequestIdRef.current !== null &&
        request.requestId <= handledModuleNavigationRequestIdRef.current)
    )
      return
    const contextMatches =
      createRightSidebarWorkspaceSessionKey(request.workspaceKey, request.workspacePath) ===
        createRightSidebarWorkspaceSessionKey(workspaceKey, workspacePath) &&
      (request.conversationId === undefined ||
        (request.conversationId ?? null) === (activeConversationId ?? null))
    if (!contextMatches) {
      handledModuleNavigationRequestIdRef.current = request.requestId
      return
    }
    const availability = moduleAvailability[request.moduleId]
    if (availability === 'checking') return
    handledModuleNavigationRequestIdRef.current = request.requestId
    if (availability === 'available') openModule(request.moduleId)
  }, [
    activeConversationId,
    moduleAvailability,
    moduleNavigationRequest,
    openModule,
    workspaceKey,
    workspacePath
  ])

  useEffect(() => {
    if (
      !agentNavigationRequest ||
      handledAgentNavigationRequestIdRef.current === agentNavigationRequest.requestId
    ) {
      return
    }
    if (
      agentNavigationRequest.rootConversationId !== activeConversationId ||
      !childAgents.some((agent) => agent.agentId === agentNavigationRequest.agentId)
    ) {
      handledAgentNavigationRequestIdRef.current = agentNavigationRequest.requestId
      return
    }
    handledAgentNavigationRequestIdRef.current = agentNavigationRequest.requestId
    openPlatformModule('agent-center', {
      agentId: agentNavigationRequest.agentId,
      kind: 'agent-center',
      rootConversationId: agentNavigationRequest.rootConversationId,
      view: 'detail'
    })
  }, [activeConversationId, agentNavigationRequest, childAgents, openPlatformModule])

  useEffect(() => {
    if (!activeConversationId) return
    for (const page of pages) {
      if (page.moduleId !== 'agent-center') continue
      if (
        page.moduleState?.kind === 'agent-center' &&
        page.moduleState.rootConversationId === activeConversationId
      ) {
        continue
      }
      updatePage(page.id, {
        moduleState: {
          kind: 'agent-center',
          rootConversationId: activeConversationId,
          view: 'list'
        },
        title: t('rightSidebar.agentCenter')
      })
    }
  }, [activeConversationId, pages, t, updatePage])

  useEffect(() => {
    if (
      !reviewNavigationRequest ||
      handledReviewNavigationRequestIdRef.current === reviewNavigationRequest.requestId
    ) {
      return
    }
    if (reviewNavigationRequest.projectId !== workspaceKey) {
      handledReviewNavigationRequestIdRef.current = reviewNavigationRequest.requestId
      return
    }
    const availability = moduleAvailability['git-review']
    if (availability === 'checking') return
    handledReviewNavigationRequestIdRef.current = reviewNavigationRequest.requestId
    if (availability !== 'available') return
    openPlatformModule('git-review', reviewNavigationRequest)
  }, [moduleAvailability, openPlatformModule, reviewNavigationRequest, workspaceKey])

  return (
    <aside
      className={`right-sidebar${hasOpenPages ? '' : ' right-sidebar--home'}${
        isMaximized ? ' right-sidebar--maximized' : ''
      }`}
      aria-label={t('app.rightSidebar')}
    >
      <header
        className={`right-sidebar__toolbar${hasOpenPages ? '' : ' right-sidebar__toolbar--home'}`}
        data-drag-region
      >
        {hasOpenPages ? (
          <RightSidebarTabStrip
            activePageId={activePageId}
            automationPageIds={automationPageIds}
            availableModules={availableModules}
            isMenuOpen={isModuleMenuOpen}
            modules={modules}
            onActivatePage={(pageId) => {
              if (pageId !== activePageId && hasForegroundBrowser) clearCurrentBrowserSelection()
              activatePage(pageId)
            }}
            onClosePage={(pageId) => {
              if (pageId === activePageId && hasForegroundBrowser) clearCurrentBrowserSelection()
              closePage(pageId)
            }}
            onMenuOpenChange={setIsModuleMenuOpen}
            onOpenModule={openModule}
            pages={pages}
            t={t}
          />
        ) : (
          <div className="right-sidebar__toolbar-spacer" data-drag-region />
        )}

        <div className="right-sidebar__toolbar-actions">
          {isMaximized && maximizedToolbarControls}
          <button
            className="right-sidebar__icon-button"
            type="button"
            aria-label={maximizeLabel}
            aria-pressed={isMaximized}
            onClick={onToggleMaximized}
            title={maximizeLabel}
          >
            {isMaximized ? <RestoreFromMaximizedIcon /> : <Maximize aria-hidden="true" />}
          </button>
        </div>
      </header>

      <WorkspaceFileTreeSessionsProvider
        projectIds={fileTreeProjectIds}
        projectRevisions={projectWorkspaceRevisions}
      >
        <div className="right-sidebar__content">
          <RightSidebarRuntimeContext.Provider value={runtimeContext}>
            {hasOpenPages ? (
              <RightSidebarPageStack
                activePageId={activePageId}
                automationPageIds={automationPageIds}
                availability={moduleAvailability}
                documentVisible={documentVisible}
                modules={modules}
                onOpenPage={openRelatedPage}
                onPageUpdate={updatePage}
                onSurfaceFocus={closeTransientUi}
                pages={pages}
                sidebarVisible={sidebarVisible}
                t={t}
              />
            ) : (
              <RightSidebarHome modules={availableModules} onOpenModule={openModule} />
            )}
          </RightSidebarRuntimeContext.Provider>
        </div>
      </WorkspaceFileTreeSessionsProvider>
    </aside>
  )
})
