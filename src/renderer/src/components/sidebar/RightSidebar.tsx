// Renderer UI.
// Hosts the right-sidebar module integration shell and keep-alive tab surface.

import { Suspense, lazy, useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Maximize, Plus, X } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { RightSidebarHome } from '../../features/rightSidebar/RightSidebarHome'
import { RightSidebarModulePicker } from '../../features/rightSidebar/RightSidebarModulePicker'
import { RightSidebarPageStack } from '../../features/rightSidebar/RightSidebarPageStack'
import type { BrowserPageMetadata } from '../../features/browser/browserClient'
import {
  RIGHT_SIDEBAR_MODULES,
  getRightSidebarModule
} from '../../features/rightSidebar/rightSidebarModules'
import type {
  RightSidebarModuleId,
  RightSidebarPage
} from '../../features/rightSidebar/rightSidebarTypes'
import './RightSidebar.css'

const TerminalPanel = lazy(async () => {
  const module = await import('../../features/terminal/TerminalPanel')
  return { default: module.TerminalPanel }
})

const BrowserPanel = lazy(async () => {
  const module = await import('../../features/browser/BrowserPanel')
  return { default: module.BrowserPanel }
})

interface RightSidebarProps {
  isMaximized: boolean
  maximizedToolbarControls?: ReactNode
  onToggleMaximized: () => void
  workspaceName?: string | null
  workspacePath?: string
}

function createPageId(moduleId: RightSidebarModuleId) {
  const randomValue = Math.random().toString(36).slice(2, 8)
  return `${moduleId}-${Date.now().toString(36)}-${randomValue}`
}

function RestoreFromMaximizedIcon() {
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

function getWorkspaceTabTitle(
  workspacePath: string | undefined,
  workspaceName: string | null | undefined
) {
  const pathName = workspacePath?.split(/[\\/]/).filter(Boolean).at(-1)?.trim()

  return pathName || workspaceName || null
}

function getWorkspaceKey(
  workspacePath: string | undefined,
  workspaceName: string | null | undefined
) {
  return workspacePath || workspaceName || 'home'
}

function getTerminalPageTitle(
  pages: RightSidebarPage[],
  workspacePath: string | undefined,
  workspaceName: string | null | undefined,
  fallbackTitle: string
) {
  const workspaceKey = getWorkspaceKey(workspacePath, workspaceName)
  const baseTitle = getWorkspaceTabTitle(workspacePath, workspaceName) || fallbackTitle
  const existingCount = pages.filter(
    (page) => page.moduleId === 'terminal' && page.workspaceKey === workspaceKey
  ).length

  return existingCount === 0 ? baseTitle : `${baseTitle} (${existingCount})`
}

function getPageTitle(
  moduleId: RightSidebarModuleId,
  pages: RightSidebarPage[],
  workspacePath: string | undefined,
  workspaceName: string | null | undefined,
  fallbackTitle: string
) {
  if (moduleId === 'terminal') {
    return getTerminalPageTitle(pages, workspacePath, workspaceName, fallbackTitle)
  }

  const sequence = pages.filter((page) => page.moduleId === moduleId).length
  return sequence > 0 ? `${fallbackTitle} (${sequence})` : fallbackTitle
}

export function RightSidebar({
  isMaximized,
  maximizedToolbarControls,
  onToggleMaximized,
  workspaceName,
  workspacePath
}: RightSidebarProps) {
  const { t } = useFrontendConfig()
  const moduleMenuRef = useRef<HTMLDivElement>(null)
  const moduleMenuButtonRef = useRef<HTMLButtonElement>(null)
  const [pages, setPages] = useState<RightSidebarPage[]>([])
  const [activePageId, setActivePageId] = useState<string | null>(null)
  const [isModuleMenuOpen, setIsModuleMenuOpen] = useState(false)
  const [moduleMenuPosition, setModuleMenuPosition] = useState({ top: 0, left: 0 })
  const hasOpenPages = pages.length > 0
  const maximizeLabel = isMaximized ? t('rightSidebar.restore') : t('rightSidebar.maximize')

  useEffect(() => {
    if (!isModuleMenuOpen) return

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (moduleMenuRef.current?.contains(target)) return
      if (moduleMenuButtonRef.current?.contains(target)) return

      setIsModuleMenuOpen(false)
    }

    document.addEventListener('pointerdown', handlePointerDown, true)
    return () => document.removeEventListener('pointerdown', handlePointerDown, true)
  }, [isModuleMenuOpen])

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
      const module = getRightSidebarModule(moduleId)
      if (!module?.isEnabled) return

      const pageId = createPageId(moduleId)
      setPages((currentPages) => {
        const fallbackTitle =
          moduleId === 'terminal'
            ? t('terminal.title')
            : moduleId === 'browser'
              ? t('browser.newTab')
              : t(module.titleKey)
        const page: RightSidebarPage = {
          id: pageId,
          moduleId,
          title: getPageTitle(moduleId, currentPages, workspacePath, workspaceName, fallbackTitle),
          workspaceKey:
            moduleId === 'terminal' ? getWorkspaceKey(workspacePath, workspaceName) : null,
          workspacePath: moduleId === 'terminal' ? workspacePath : undefined
        }

        return [...currentPages, page]
      })
      setActivePageId(pageId)
      setIsModuleMenuOpen(false)
    },
    [t, workspaceName, workspacePath]
  )

  const closePage = useCallback((pageId: string) => {
    setPages((currentPages) => {
      const pageIndex = currentPages.findIndex((page) => page.id === pageId)
      if (pageIndex < 0) return currentPages

      const nextPages = currentPages.filter((page) => page.id !== pageId)
      setActivePageId((currentActivePageId) => {
        if (currentActivePageId !== pageId) {
          return nextPages.some((page) => page.id === currentActivePageId)
            ? currentActivePageId
            : (nextPages[0]?.id ?? null)
        }

        return nextPages[Math.min(pageIndex, nextPages.length - 1)]?.id ?? null
      })
      return nextPages
    })
  }, [])

  const updateBrowserPageMetadata = useCallback((pageId: string, metadata: BrowserPageMetadata) => {
    setPages((currentPages) =>
      currentPages.map((page) => {
        if (page.id !== pageId || page.moduleId !== 'browser') return page

        return {
          ...page,
          iconUrl: metadata.iconUrl,
          title: metadata.title?.trim() || page.title
        }
      })
    )
  }, [])

  const renderPageContent = (page: RightSidebarPage, isActive: boolean) => {
    if (page.moduleId === 'terminal') {
      return (
        <Suspense
          fallback={
            <div className="right-sidebar__panel-loading">{t('terminal.status.starting')}</div>
          }
        >
          <TerminalPanel initialCwd={page.workspacePath} isActive={isActive} />
        </Suspense>
      )
    }

    if (page.moduleId === 'browser') {
      return (
        <Suspense
          fallback={<div className="right-sidebar__panel-loading">{t('browser.title')}</div>}
        >
          <BrowserPanel
            isActive={isActive}
            onPageMetadataChange={(metadata) => updateBrowserPageMetadata(page.id, metadata)}
            pageId={page.id}
          />
        </Suspense>
      )
    }

    return <RightSidebarHome modules={RIGHT_SIDEBAR_MODULES} onOpenModule={openModule} />
  }

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
                const module = getRightSidebarModule(page.moduleId)
                const Icon = module?.icon
                const isActive = page.id === activePageId
                const iconUrl = page.iconUrl?.trim()

                return (
                  <div
                    className="right-sidebar__tab-shell"
                    data-active={isActive ? 'true' : undefined}
                    key={page.id}
                  >
                    <button
                      className="right-sidebar__tab"
                      type="button"
                      role="tab"
                      aria-selected={isActive}
                      data-active={isActive ? 'true' : undefined}
                      onClick={() => setActivePageId(page.id)}
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

              <div className="right-sidebar__module-menu-anchor" ref={moduleMenuRef}>
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
                        modules={RIGHT_SIDEBAR_MODULES}
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

      <div className="right-sidebar__content">
        {hasOpenPages ? (
          <RightSidebarPageStack
            activePageId={activePageId}
            pages={pages}
            renderPage={renderPageContent}
          />
        ) : (
          <RightSidebarHome modules={RIGHT_SIDEBAR_MODULES} onOpenModule={openModule} />
        )}
      </div>
    </aside>
  )
}
