// Shared pin / rename / archive conversation menu used by the sidebar and title bar.
import { Archive, Mail, PencilLine, Pin } from 'lucide-react'
import type { Ref } from 'react'
import './LeftSidebar.css'

export type ConversationActionsMenuPosition = { x: number; y: number }

interface ConversationActionsMenuProps {
  archiveLabel: string
  canMarkUnread?: boolean
  isPinned: boolean
  markUnreadLabel?: string
  menuPosition: ConversationActionsMenuPosition
  menuRef?: Ref<HTMLDivElement | null>
  onArchive: () => void
  onClose: () => void
  onMarkUnread?: () => void
  onRename: () => void
  onTogglePin: () => void
  pinLabel: string
  renameLabel: string
  showMarkUnread?: boolean
  unpinLabel: string
}

export function ConversationActionsMenu({
  archiveLabel,
  canMarkUnread = false,
  isPinned,
  markUnreadLabel,
  menuPosition,
  menuRef,
  onArchive,
  onClose,
  onMarkUnread,
  onRename,
  onTogglePin,
  pinLabel,
  renameLabel,
  showMarkUnread = false,
  unpinLabel
}: ConversationActionsMenuProps) {
  return (
    <div
      className="left-sidebar__conversation-menu"
      role="menu"
      ref={menuRef}
      style={{ left: menuPosition.x, top: menuPosition.y }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          onClose()
        }
      }}
    >
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        onClick={() => {
          onTogglePin()
          onClose()
        }}
      >
        <Pin aria-hidden="true" />
        <span>{isPinned ? unpinLabel : pinLabel}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        onClick={() => {
          onRename()
          onClose()
        }}
      >
        <PencilLine aria-hidden="true" />
        <span>{renameLabel}</span>
      </button>
      <button
        className="left-sidebar__project-menu-item"
        type="button"
        role="menuitem"
        onClick={() => {
          onArchive()
          onClose()
        }}
      >
        <Archive aria-hidden="true" />
        <span>{archiveLabel}</span>
      </button>
      {showMarkUnread ? (
        <button
          className="left-sidebar__project-menu-item"
          type="button"
          role="menuitem"
          disabled={!canMarkUnread}
          onClick={() => {
            if (!canMarkUnread) return

            onMarkUnread?.()
            onClose()
          }}
        >
          <Mail aria-hidden="true" />
          <span>{markUnreadLabel}</span>
        </button>
      ) : null}
    </div>
  )
}
