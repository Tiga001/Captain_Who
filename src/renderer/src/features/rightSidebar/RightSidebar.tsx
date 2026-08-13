import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Maximize, Plus, X } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { WorkspaceFileTreeSessionsProvider } from '../files/WorkspaceFileTreeSessions'
import { RightSidebarHome } from './RightSidebarHome'
import { RightSidebarModulePicker } from './RightSidebarModulePicker'
import { RightSidebarPageStack } from './RightSidebarPageStack'
import { RightSidebarRuntimeContext } from './RightSidebarRuntimeContext'
import { useRightSidebarDocumentVisibility } from './rightSidebarActivity'
import { AGENT_CENTER_RIGHT_SIDEBAR_MODULE, RIGHT_SIDEBAR_MODULES } from './rightSidebarModules'
import type { CollaborationStoreSnapshot } from '../agentCollaboration/collaborationStore'
import type {
  RightSidebarCapabilities,
  AgentObserverRenderContext,
  RightSidebarAgentNavigationRequest,
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarReviewNavigationRequest
} from './rightSidebarTypes'
import { useRightSidebarPlatform } from './useRightSidebarPlatform'
import './RightSidebar.css'

interface RightSidebarProps {
  activeConversationId?: string | null
  agentNavigationRequest?: RightSidebarAgentNavigationRequest | null
  capabilities?: RightSidebarCapabilities
  collaborationSnapshot?: CollaborationStoreSnapshot | null
  isMaximized: boolean
  isOpen: boolean
  isWorkspaceVisible?: boolean
  maximizedToolbarControls?: ReactNode
  modules?: RightSidebarModuleDefinition[]
  onOpenAgentTemplates?: () => void
  onToggleMaximized: () => void
  reviewNavigationRequest?: RightSidebarReviewNavigationRequest | null
  renderAgentObserver?: (context: AgentObserverRenderContext) => ReactNode
  workspaceKey?: string | null
  workspaceKeys?: readonly string[]
  workspaceName?: string | null
  workspacePath?: string
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
  capabilities,
  collaborationSnapshot = null,
  isMaximized,
  isOpen,
  isWorkspaceVisible = true,
  maximizedToolbarControls,
  modules: configuredModules = RIGHT_SIDEBAR_MODULES,
  onOpenAgentTemplates,
  onToggleMaximized,
  reviewNavigationRequest,
  renderAgentObserver,
  workspaceKey,
  workspaceKeys,
  workspaceName,
  workspacePath
}: RightSidebarProps): ReactNode {
  const { t } = useFrontendConfig()
  const documentVisible = useRightSidebarDocumentVisibility()
  const handledReviewNavigationRequestIdRef = useRef<number | null>(null)
  const handledAgentNavigationRequestIdRef = useRef<number | null>(null)
  const moduleMenuRef = useRef<HTMLDivElement>(null)
  const moduleMenuButtonRef = useRef<HTMLButtonElement>(null)
  const [isModuleMenuOpen, setIsModuleMenuOpen] = useState(false)
  const [moduleMenuPosition, setModuleMenuPosition] = useState({ top: 0, left: 0 })
  const childAgents = useMemo(() => {
    const tree = collaborationSnapshot?.tree
    if (!activeConversationId || tree?.rootConversationId !== activeConversationId) return []
    return tree.agents.filter((agent) => agent.parentAgentId !== null)
  }, [activeConversationId, collaborationSnapshot?.tree])
  const activeChildCount = childAgents.filter((agent) =>
    ['queued', 'running', 'waiting_approval'].includes(agent.displayStatus)
  ).length
  const modules = useMemo(() => {
    const withoutAgentCenter = configuredModules.filter((module) => module.id !== 'agent-center')
    return childAgents.length > 0
      ? [
          ...withoutAgentCenter,
          { ...AGENT_CENTER_RIGHT_SIDEBAR_MODULE, badge: activeChildCount || undefined }
        ]
      : withoutAgentCenter
  }, [activeChildCount, childAgents.length, configuredModules])
  const {
    activatePage,
    activePageId,
    availableModules,
    closePage,
    moduleAvailability,
    openModule: openPlatformModule,
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
  const maximizeLabel = isMaximized ? t('rightSidebar.restore') : t('rightSidebar.maximize')
  const runtimeContext = useMemo(
    () => ({
      activeConversationId: activeConversationId ?? null,
      activeWorkspaceKey: workspaceKey ?? null,
      collaborationSnapshot,
      onOpenAgentTemplates,
      renderAgentObserver
    }),
    [
      activeConversationId,
      collaborationSnapshot,
      onOpenAgentTemplates,
      renderAgentObserver,
      workspaceKey
    ]
  )

  const closeTransientUi = useCallback(() => {
    setIsModuleMenuOpen(false)
  }, [])

  useEffect(() => {
    if (!sidebarVisible) closeTransientUi()
  }, [closeTransientUi, sidebarVisible])

  useEffect(() => {
    if (!isModuleMenuOpen) return

    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (moduleMenuRef.current?.contains(target)) return
      if (moduleMenuButtonRef.current?.contains(target)) return

      closeTransientUi()
    }

    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [closeTransientUi, isModuleMenuOpen])

  const updateModuleMenuPosition = useCallback(() => {
    const button = moduleMenuButtonRef.current
    if (!button) return

    const rect = button.getBoundingClientRect()
    const menuWidth = 280
    const left = Math.min(Math.max(12, rect.left), Math.max(12, window.innerWidth - menuWidth - 12))

    setModuleMenuPosition({
      top: rect.bottom + 8,
      left
    })
  }, [])

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
          <div className="right-sidebar__tab-scroll" data-drag-region>
            <div
              className="right-sidebar__tabs"
              role="tablist"
              aria-label={t('rightSidebar.openTabs')}
            >
              {pages.map((page) => {
                const module = modules.find((candidate) => candidate.id === page.moduleId)
                const Icon = module?.icon
                const isSelected = page.id === activePageId
                const iconUrl = page.iconUrl?.trim()

                return (
                  <div
                    className="right-sidebar__tab-shell"
                    data-active={isSelected ? 'true' : undefined}
                    key={page.id}
                  >
                    <button
                      className="right-sidebar__tab"
                      type="button"
                      role="tab"
                      aria-selected={isSelected}
                      data-active={isSelected ? 'true' : undefined}
                      onClick={() => activatePage(page.id)}
                    >
                      {iconUrl ? (
                        <img
                          className="right-sidebar__tab-favicon"
                          src={iconUrl}
                          alt=""
                          aria-hidden="true"
                          onError={(event) => {
                            event.currentTarget.style.display = 'none'
                          }}
                        />
                      ) : (
                        Icon && <Icon aria-hidden="true" />
                      )}
                      <span>{page.title}</span>
                    </button>
                    <button
                      className="right-sidebar__tab-close"
                      type="button"
                      aria-label={t('rightSidebar.closeTab')}
                      title={t('rightSidebar.closeTab')}
                      onClick={(event) => {
                        event.stopPropagation()
                        closePage(page.id)
                      }}
                    >
                      <X aria-hidden="true" />
                    </button>
                  </div>
                )
              })}

              <div className="right-sidebar__module-menu-anchor">
                <button
                  ref={moduleMenuButtonRef}
                  className="right-sidebar__new-tab-button"
                  type="button"
                  aria-label={t('rightSidebar.newPanel')}
                  aria-expanded={isModuleMenuOpen}
                  onMouseDown={(event) => {
                    event.preventDefault()
                    event.stopPropagation()
                  }}
                  onPointerDown={(event) => {
                    event.stopPropagation()
                  }}
                  onClick={(event) => {
                    event.preventDefault()
                    event.stopPropagation()
                    updateModuleMenuPosition()
                    setIsModuleMenuOpen((isOpen) => !isOpen)
                  }}
                >
                  <Plus aria-hidden="true" />
                </button>
                {isModuleMenuOpen &&
                  createPortal(
                    <div ref={moduleMenuRef}>
                      <RightSidebarModulePicker
                        modules={availableModules}
                        onOpenModule={openModule}
                        style={moduleMenuPosition}
                      />
                    </div>,
                    document.body
                  )}
              </div>
            </div>
          </div>
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

      <WorkspaceFileTreeSessionsProvider projectIds={fileTreeProjectIds}>
        <div className="right-sidebar__content">
          <RightSidebarRuntimeContext.Provider value={runtimeContext}>
            {hasOpenPages ? (
              <RightSidebarPageStack
                activePageId={activePageId}
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
