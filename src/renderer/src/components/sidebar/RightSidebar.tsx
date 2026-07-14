import { useCallback, useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Maximize, Plus, X } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { RightSidebarHome } from '../../features/rightSidebar/RightSidebarHome'
import { RightSidebarModulePicker } from '../../features/rightSidebar/RightSidebarModulePicker'
import { RightSidebarPageStack } from '../../features/rightSidebar/RightSidebarPageStack'
import {
  RIGHT_SIDEBAR_MODULES,
  getRightSidebarModule
} from '../../features/rightSidebar/rightSidebarModules'
import type { RightSidebarModuleId } from '../../features/rightSidebar/rightSidebarTypes'
import { useRightSidebarPlatform } from '../../features/rightSidebar/useRightSidebarPlatform'
import './RightSidebar.css'

interface RightSidebarProps {
  isMaximized: boolean
  maximizedToolbarControls?: ReactNode
  onToggleMaximized: () => void
  workspaceKey?: string | null
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

export function RightSidebar({
  isMaximized,
  maximizedToolbarControls,
  onToggleMaximized,
  workspaceKey,
  workspaceName,
  workspacePath
}: RightSidebarProps): ReactNode {
  const { t } = useFrontendConfig()
  const moduleMenuRef = useRef<HTMLDivElement>(null)
  const moduleMenuButtonRef = useRef<HTMLButtonElement>(null)
  const [isModuleMenuOpen, setIsModuleMenuOpen] = useState(false)
  const [moduleMenuPosition, setModuleMenuPosition] = useState({ top: 0, left: 0 })
  const {
    activatePage,
    activePageId,
    closePage,
    openModule: openPlatformModule,
    pages,
    updatePage
  } = useRightSidebarPlatform({
    t,
    workspaceKey,
    workspaceName,
    workspacePath
  })
  const hasOpenPages = pages.length > 0
  const maximizeLabel = isMaximized ? t('rightSidebar.restore') : t('rightSidebar.maximize')

  const closeTransientUi = useCallback(() => {
    setIsModuleMenuOpen(false)
  }, [])

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
      openPlatformModule(moduleId)
      closeTransientUi()
    },
    [closeTransientUi, openPlatformModule]
  )

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
            onPageUpdate={updatePage}
            onSurfaceFocus={closeTransientUi}
            pages={pages}
            t={t}
          />
        ) : (
          <RightSidebarHome modules={RIGHT_SIDEBAR_MODULES} onOpenModule={openModule} />
        )}
      </div>
    </aside>
  )
}
