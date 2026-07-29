// Conversation row presentation and local context-menu behavior.
import { Archive, Mail, PencilLine, Pin } from 'lucide-react'
import { useRef, useState } from 'react'
import type { AppLanguage } from '../../../config/frontendTranslations'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { formatConversationAge } from './leftSidebarUtils'

function ConversationPinIcon({ filled }: { filled: boolean }) {
  return (
    <svg
      className="left-sidebar__conversation-pin-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M12 17v5" />
      <path
        d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"
        fill={filled ? 'currentColor' : 'none'}
      />
    </svg>
  )
}

interface ConversationRowProps {
  activeConversationId: string | null
  archiveLabel: string
  conversation: ChatConversation
  justNow: string
  language: AppLanguage
  markUnreadLabel: string
  now: number
  onArchiveConversation: (conversationId: string) => void
  onMarkConversationUnread: (conversationId: string) => void
  onRenameConversation: (conversation: ChatConversation) => void
  onSelectConversation: (conversationId: string) => void
  onTogglePinConversation: (conversationId: string) => void
  pinLabel: string
  processingLabel: string
  renameLabel: string
  unreadLabel: string
  unpinLabel: string
  waitingApprovalLabel: string
  nested?: boolean
}

export function ConversationRow({
  activeConversationId,
  archiveLabel,
  conversation,
  justNow,
  language,
  markUnreadLabel,
  now,
  onArchiveConversation,
  onMarkConversationUnread,
  onRenameConversation,
  onSelectConversation,
  onTogglePinConversation,
  pinLabel,
  processingLabel,
  renameLabel,
  unreadLabel,
  unpinLabel,
  waitingApprovalLabel,
  nested = false
}: ConversationRowProps) {
  const conversationMenuRef = useRef<HTMLDivElement>(null)
  const [menuPosition, setMenuPosition] = useState<{ x: number; y: number } | null>(null)
  const isPinned = Boolean(conversation.pinnedAt)
  const isPending = conversation.messages.some(
    (message) => message.role === 'assistant' && message.status === 'pending'
  )
  const isWaitingForApproval = conversation.messages.some(
    (message) => message.role === 'assistant' && message.agentRun?.status === 'waiting_for_approval'
  )
  const showWaitingApprovalBadge = isWaitingForApproval && conversation.id !== activeConversationId
  const isUnread = Boolean(
    conversation.unreadAt && conversation.id !== activeConversationId && !isPending
  )
  const canMarkUnread =
    conversation.id !== activeConversationId && !isPending && !conversation.unreadAt
  const isConversationMenuOpen = Boolean(menuPosition)

  const closeConversationMenu = () => setMenuPosition(null)
  useDismissOnOutsidePointer(conversationMenuRef, Boolean(menuPosition), closeConversationMenu)

  return (
    <div
      className={`left-sidebar__conversation-row${nested ? ' left-sidebar__conversation-row--nested' : ''}`}
      data-active={conversation.id === activeConversationId || undefined}
      data-awaiting-approval={showWaitingApprovalBadge || undefined}
      data-menu-open={isConversationMenuOpen || undefined}
      data-pending={isPending || undefined}
      onContextMenu={(event) => {
        event.preventDefault()
        setMenuPosition({
          x: Math.max(12, Math.min(event.clientX, window.innerWidth - 238)),
          y: Math.max(12, Math.min(event.clientY, window.innerHeight - 190))
        })
      }}
    >
      <button
        className="left-sidebar__conversation-main"
        type="button"
        onClick={() => onSelectConversation(conversation.id)}
      >
        <span className="left-sidebar__conversation-name">{conversation.title}</span>
      </button>

      <span className="left-sidebar__conversation-age">
        {showWaitingApprovalBadge ? (
          <>
            <span className="left-sidebar__conversation-approval-badge">
              {waitingApprovalLabel}
            </span>
            <span className="mc-processing-spinner" aria-label={processingLabel} />
          </>
        ) : isPending ? (
          <span className="mc-processing-spinner" aria-label={processingLabel} />
        ) : isUnread ? (
          <span className="left-sidebar__conversation-unread-dot" aria-label={unreadLabel} />
        ) : (
          formatConversationAge(conversation.updatedAt, now, language, justNow)
        )}
      </span>

      <div className="left-sidebar__conversation-item-actions" aria-label={archiveLabel}>
        <Tooltip content={isPinned ? unpinLabel : pinLabel}>
          <button
            className="left-sidebar__conversation-item-action"
            type="button"
            data-pinned={isPinned || undefined}
            aria-label={isPinned ? unpinLabel : pinLabel}
            disabled={isConversationMenuOpen}
            onClick={() => onTogglePinConversation(conversation.id)}
          >
            <ConversationPinIcon filled={isPinned} />
          </button>
        </Tooltip>
        <Tooltip content={archiveLabel}>
          <button
            className="left-sidebar__conversation-item-action"
            type="button"
            aria-label={archiveLabel}
            disabled={isConversationMenuOpen}
            onClick={() => onArchiveConversation(conversation.id)}
          >
            <Archive aria-hidden="true" />
          </button>
        </Tooltip>
      </div>

      {menuPosition && (
        <div
          className="left-sidebar__conversation-menu"
          role="menu"
          ref={conversationMenuRef}
          style={{ left: menuPosition.x, top: menuPosition.y }}
        >
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            onClick={() => {
              onTogglePinConversation(conversation.id)
              closeConversationMenu()
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
              onRenameConversation(conversation)
              closeConversationMenu()
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
              onArchiveConversation(conversation.id)
              closeConversationMenu()
            }}
          >
            <Archive aria-hidden="true" />
            <span>{archiveLabel}</span>
          </button>
          <button
            className="left-sidebar__project-menu-item"
            type="button"
            role="menuitem"
            disabled={!canMarkUnread}
            onClick={() => {
              if (!canMarkUnread) return

              onMarkConversationUnread(conversation.id)
              closeConversationMenu()
            }}
          >
            <Mail aria-hidden="true" />
            <span>{markUnreadLabel}</span>
          </button>
        </div>
      )}
    </div>
  )
}
