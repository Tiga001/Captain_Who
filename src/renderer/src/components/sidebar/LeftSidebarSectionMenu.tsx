// Section action menu for sidebar ordering, sorting, and bulk archive actions.
import {
  Archive,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronDown,
  Clock3,
  NotebookText,
  PencilLine
} from 'lucide-react'
import type { RefObject } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type {
  SidebarConversationSort,
  SidebarProjectSort
} from '../../features/storage/storageClient'
import type {
  BulkArchiveScope,
  SidebarMenuPosition,
  SidebarSectionSubmenu
} from './leftSidebarTypes'

interface LeftSidebarSectionMenuProps {
  archiveCount: number
  archiveScope: BulkArchiveScope
  conversationSort: SidebarConversationSort
  isMoveDown: boolean
  menuPosition: SidebarMenuPosition | null
  onRequestBulkArchive: (scope: BulkArchiveScope) => void
  onSetConversationSort: (sort: SidebarConversationSort) => void
  onSetProjectSort: (sort: SidebarProjectSort) => void
  onSubmenuChange: (submenu: SidebarSectionSubmenu | null) => void
  onToggleSectionOrder: () => void
  openSubmenu: SidebarSectionSubmenu | null
  projectSort: SidebarProjectSort
  sectionMenuRef: RefObject<HTMLDivElement | null>
  t: (key: TranslationKey) => string
}

export function LeftSidebarSectionMenu({
  archiveCount,
  archiveScope,
  conversationSort,
  isMoveDown,
  menuPosition,
  onRequestBulkArchive,
  onSetConversationSort,
  onSetProjectSort,
  onSubmenuChange,
  onToggleSectionOrder,
  openSubmenu,
  projectSort,
  sectionMenuRef,
  t
}: LeftSidebarSectionMenuProps) {
  const MoveIcon = isMoveDown ? ArrowDown : ArrowUp

  return (
    <div
      className="left-sidebar__section-menu"
      role="menu"
      ref={sectionMenuRef}
      style={menuPosition ?? undefined}
    >
      <button
        className="left-sidebar__section-menu-item"
        type="button"
        role="menuitem"
        disabled={archiveCount === 0}
        onClick={() => onRequestBulkArchive(archiveScope)}
      >
        <Archive aria-hidden="true" />
        <span>{t('sidebar.archiveAllChats')}</span>
      </button>

      <div className="left-sidebar__section-menu-divider" />

      <button
        className="left-sidebar__section-menu-item"
        type="button"
        role="menuitem"
        data-open={openSubmenu === 'organize' || undefined}
        onMouseEnter={() => onSubmenuChange('organize')}
        onClick={() => onSubmenuChange(openSubmenu === 'organize' ? null : 'organize')}
      >
        <NotebookText aria-hidden="true" />
        <span>{t('sidebar.organizeSidebar')}</span>
        <ChevronDown className="left-sidebar__section-menu-item__chevron" aria-hidden="true" />
      </button>

      <button
        className="left-sidebar__section-menu-item"
        type="button"
        role="menuitem"
        data-open={openSubmenu === 'sort' || undefined}
        onMouseEnter={() => onSubmenuChange('sort')}
        onClick={() => onSubmenuChange(openSubmenu === 'sort' ? null : 'sort')}
      >
        <Clock3 aria-hidden="true" />
        <span>{t('sidebar.sortBy')}</span>
        <ChevronDown className="left-sidebar__section-menu-item__chevron" aria-hidden="true" />
      </button>

      {openSubmenu === 'organize' && (
        <div className="left-sidebar__section-submenu" role="menu">
          <button
            className="left-sidebar__section-menu-item"
            type="button"
            role="menuitem"
            onClick={() => onSetProjectSort('created')}
          >
            <NotebookText aria-hidden="true" />
            <span>{t('sidebar.organizeByProject')}</span>
            {projectSort === 'created' && (
              <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
            )}
          </button>
          <button
            className="left-sidebar__section-menu-item"
            type="button"
            role="menuitem"
            onClick={() => onSetProjectSort('recent')}
          >
            <NotebookText aria-hidden="true" />
            <span>{t('sidebar.organizeByRecentProject')}</span>
            {projectSort === 'recent' && (
              <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
            )}
          </button>
          <button
            className="left-sidebar__section-menu-item"
            type="button"
            role="menuitem"
            onClick={onToggleSectionOrder}
          >
            <MoveIcon aria-hidden="true" />
            <span>{isMoveDown ? t('sidebar.moveDown') : t('sidebar.moveUp')}</span>
          </button>
        </div>
      )}

      {openSubmenu === 'sort' && (
        <div className="left-sidebar__section-submenu" role="menu">
          <button
            className="left-sidebar__section-menu-item"
            type="button"
            role="menuitem"
            onClick={() => onSetConversationSort('created')}
          >
            <Clock3 aria-hidden="true" />
            <span>{t('sidebar.sortCreated')}</span>
            {conversationSort === 'created' && (
              <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
            )}
          </button>
          <button
            className="left-sidebar__section-menu-item"
            type="button"
            role="menuitem"
            onClick={() => onSetConversationSort('updated')}
          >
            <PencilLine aria-hidden="true" />
            <span>{t('sidebar.sortUpdated')}</span>
            {conversationSort === 'updated' && (
              <Check className="left-sidebar__section-menu-check" aria-hidden="true" />
            )}
          </button>
        </div>
      )}
    </div>
  )
}
