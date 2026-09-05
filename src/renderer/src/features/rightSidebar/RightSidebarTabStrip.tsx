import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Plus, X } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import { RightSidebarModulePicker } from './RightSidebarModulePicker'
import type {
  RightSidebarModuleDefinition,
  RightSidebarModuleId,
  RightSidebarPage
} from './rightSidebarTypes'

interface RightSidebarTabStripProps {
  activePageId: string | null
  automationPageId?: string
  availableModules: RightSidebarModuleDefinition[]
  dragRegion?: boolean
  isMenuOpen: boolean
  menuPlacement?: 'top' | 'bottom'
  modules: RightSidebarModuleDefinition[]
  onActivatePage: (pageId: string) => void
  onClosePage: (pageId: string) => void
  onMenuOpenChange: (open: boolean) => void
  onOpenModule: (moduleId: RightSidebarModuleId) => void
  pages: RightSidebarPage[]
  t: Translate
}

export function RightSidebarTabStrip({
  activePageId,
  automationPageId,
  availableModules,
  dragRegion = true,
  isMenuOpen,
  menuPlacement = 'bottom',
  modules,
  onActivatePage,
  onClosePage,
  onMenuOpenChange,
  onOpenModule,
  pages,
  t
}: RightSidebarTabStripProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const [position, setPosition] = useState({ left: 0, top: 0, maxHeight: 0, width: 280 })
  const updatePosition = useCallback(() => {
    const button = buttonRef.current
    if (!button) return
    const rect = button.getBoundingClientRect()
    const padding = 12
    const gap = 8
    const width = Math.min(280, Math.max(0, window.innerWidth - padding * 2))
    const above = Math.max(0, rect.top - gap - padding)
    const below = Math.max(0, window.innerHeight - rect.bottom - gap - padding)
    const naturalHeight =
      menuRef.current?.firstElementChild?.scrollHeight ?? availableModules.length * 44 + 18
    const openAbove =
      menuPlacement === 'top'
        ? above >= naturalHeight || above >= below
        : below < naturalHeight && above > below
    const maxHeight = openAbove ? above : below
    const height = Math.min(naturalHeight, maxHeight)
    setPosition({
      left: Math.min(
        Math.max(padding, rect.left),
        Math.max(padding, window.innerWidth - width - padding)
      ),
      top: openAbove ? Math.max(padding, rect.top - gap - height) : rect.bottom + gap,
      maxHeight,
      width
    })
  }, [availableModules.length, menuPlacement])

  useLayoutEffect(() => {
    if (!isMenuOpen) return
    updatePosition()
  }, [isMenuOpen, updatePosition])

  useEffect(() => {
    if (!isMenuOpen) return
    const handlePointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return
      if (menuRef.current?.contains(event.target) || buttonRef.current?.contains(event.target))
        return
      onMenuOpenChange(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      onMenuOpenChange(false)
      buttonRef.current?.focus()
    }
    document.addEventListener('pointerdown', handlePointerDown, true)
    document.addEventListener('keydown', handleKeyDown)
    document.addEventListener('scroll', updatePosition, true)
    window.addEventListener('resize', updatePosition)
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown, true)
      document.removeEventListener('keydown', handleKeyDown)
      document.removeEventListener('scroll', updatePosition, true)
      window.removeEventListener('resize', updatePosition)
    }
  }, [isMenuOpen, onMenuOpenChange, updatePosition])

  return (
    <div className="right-sidebar__tab-scroll" data-drag-region={dragRegion || undefined}>
      <div className="right-sidebar__tabs" role="tablist" aria-label={t('rightSidebar.openTabs')}>
        {pages.map((page) => {
          const Icon = modules.find((module) => module.id === page.moduleId)?.icon
          const selected = page.id === activePageId
          const iconUrl = page.iconUrl?.trim()
          const transient =
            page.moduleId === 'files' &&
            page.moduleState?.kind === 'workspace-file' &&
            page.moduleState.tabState === 'transient'
          return (
            <div
              className="right-sidebar__tab-shell"
              data-active={selected ? 'true' : undefined}
              data-automation-active={page.id === automationPageId ? 'true' : undefined}
              key={page.id}
            >
              <button
                className="right-sidebar__tab"
                type="button"
                role="tab"
                aria-selected={selected}
                data-active={selected ? 'true' : undefined}
                data-file-preview-state={transient ? 'transient' : undefined}
                onClick={() => onActivatePage(page.id)}
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
                  onClosePage(page.id)
                }}
              >
                <span className="right-sidebar__tab-close-icon" aria-hidden="true">
                  <X />
                </span>
              </button>
            </div>
          )
        })}
        <div className="right-sidebar__module-menu-anchor">
          <button
            ref={buttonRef}
            className="right-sidebar__new-tab-button"
            type="button"
            aria-label={t('rightSidebar.newPanel')}
            aria-expanded={isMenuOpen}
            aria-haspopup="menu"
            onMouseDown={(event) => {
              event.preventDefault()
              event.stopPropagation()
            }}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={(event) => {
              event.preventDefault()
              event.stopPropagation()
              updatePosition()
              onMenuOpenChange(!isMenuOpen)
            }}
          >
            <Plus aria-hidden="true" />
          </button>
          {isMenuOpen &&
            createPortal(
              <div ref={menuRef}>
                <RightSidebarModulePicker
                  modules={availableModules}
                  onOpenModule={(moduleId) => {
                    onMenuOpenChange(false)
                    onOpenModule(moduleId)
                  }}
                  style={{ ...position, overflowY: 'auto' }}
                />
              </div>,
              document.body
            )}
        </div>
      </div>
    </div>
  )
}
