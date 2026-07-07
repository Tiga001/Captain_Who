// Project context menu actions for the left sidebar project rows.
import { Archive, FolderOpen, PencilLine, Pin, X } from 'lucide-react'
import type { RefObject } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { AppProject } from '../../config/projectConfig'
import type { SidebarMenuPosition } from './leftSidebarTypes'

interface LeftSidebarProjectMenuProps {
  isProjectPinned: boolean
  onArchiveConversations: () => void
  onRemoveProject: () => void
  onRenameProject: () => void
  onShowInFolder: () => void
  onTogglePinProject: () => void
  project: AppProject
  projectMenuPosition: SidebarMenuPosition
  projectMenuRef: RefObject<HTMLDivElement | null>
  t: (key: TranslationKey) => string
  visibleProjectConversationCount: number
}

export function LeftSidebarProjectMenu({
  isProjectPinned,
  onArchiveConversations,
  onRemoveProject,
  onRenameProject,
  onShowInFolder,
  onTogglePinProject,
  project,
  projectMenuPosition,
  projectMenuRef,
  t,
  visibleProjectConversationCount
}: LeftSidebarProjectMenuProps) {
  return (
    <div
      className="left-sidebar__project-menu"
      role="menu"
      ref={projectMenuRef}
      style={projectMenuPosition}
    >
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        onClick={onTogglePinProject}
      >
        <Pin aria-hidden="true" />
        <span>{isProjectPinned ? t('project.unpinProject') : t('project.pinProject')}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        disabled={!project.path}
        onClick={onShowInFolder}
      >
        <FolderOpen aria-hidden="true" />
        <span>{t('project.showInFolder')}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        onClick={onRenameProject}
      >
        <PencilLine aria-hidden="true" />
        <span>{t('project.renameProject')}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        disabled={visibleProjectConversationCount === 0}
        onClick={onArchiveConversations}
      >
        <Archive aria-hidden="true" />
        <span>{t('project.archiveConversations')}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item left-sidebar__project-menu-item--danger"
        type="button"
        role="menuitem"
        onClick={onRemoveProject}
      >
        <X aria-hidden="true" />
        <span>{t('project.removeProject')}</span>
      </button>
    </div>
  )
}
