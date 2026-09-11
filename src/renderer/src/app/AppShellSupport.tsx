// Small constants, queue payload types, and panel toggle controls for AppShell.
import { MoreHorizontal } from 'lucide-react'
import { useEffect, useRef, useState, type CSSProperties } from 'react'
import type { TranslationKey } from '../config/frontendTranslations'
import type { AppProject } from '../config/projectConfig'
import type { ChatMessage } from '../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { getTranslucentSidebarOpacityPercent } from '../features/storage/storageClient'
import { useDismissOnOutsidePointer } from '../hooks/useDismissOnOutsidePointer'
import { isMacOS } from '../lib/platform'
import { MainPanelBrandMark } from './shell/MainPanelBrandMark'
import { MainPanelProjectCard } from './shell/MainPanelProjectCard'
import {
  ConversationActionsMenu,
  type ConversationActionsMenuPosition
} from './shell/sidebar/ConversationActionsMenu'

export const SUPPORTS_NATIVE_FONT_SMOOTHING = isMacOS()
export const HAS_MACOS_WINDOW_CONTROLS = isMacOS()
export const STREAM_DELTA_FLUSH_MS = 80
export const STREAM_DELTA_MAX_BUFFER_CHARS = 360

export type PendingMessageSave = {
  conversationId: string
  message: ChatMessage
}

export type PendingMessageDelta = {
  conversationId: string
  delta: string
  messageId: string
  streamId?: string
  timerId: number
}

export function getAppShellPanelStyle(
  leftOpen: boolean,
  leftWidth: number,
  rightOpen: boolean,
  rightWidth: number,
  uiPreferences: UiPreferencesSnapshot,
  bottomOpen = false,
  bottomHeight = 0
): CSSProperties {
  return {
    '--left-panel-width': `${leftOpen ? leftWidth : 0}px`,
    '--right-panel-width': `${rightOpen ? rightWidth : 0}px`,
    '--bottom-panel-height': `${bottomOpen ? bottomHeight : 0}px`,
    '--mc-sidebar-translucent-opacity': getTranslucentSidebarOpacityPercent(
      uiPreferences.translucentSidebarTransparency
    )
  } as CSSProperties
}

export function getPermissionModeAvailability(uiPreferences: UiPreferencesSnapshot) {
  return {
    custom: uiPreferences.customPermissionEnabled,
    full: uiPreferences.fullPermissionEnabled
  }
}

type SidebarToggleSide = 'left' | 'right' | 'bottom'

function SidebarToggleIcon({ open, side }: { open: boolean; side: SidebarToggleSide }) {
  return (
    <span
      aria-hidden="true"
      className="panel-toggle__icon"
      data-open={open ? 'true' : 'false'}
      data-side={side}
    />
  )
}

interface PanelToggleButtonProps {
  className: string
  hasUnread?: boolean
  onClick: () => void
  open: boolean
  side: SidebarToggleSide
  showTitle?: boolean
  t: (key: TranslationKey) => string
}

export function PanelToggleButton({
  className,
  hasUnread = false,
  onClick,
  open,
  side,
  showTitle = false,
  t
}: PanelToggleButtonProps) {
  const collapseKey =
    side === 'bottom'
      ? 'app.collapseBottomPanel'
      : side === 'left'
        ? 'app.collapseLeftSidebar'
        : 'app.collapseRightSidebar'
  const expandKey =
    side === 'bottom'
      ? 'app.expandBottomPanel'
      : side === 'left'
        ? 'app.expandLeftSidebar'
        : 'app.expandRightSidebar'
  const label = open ? t(collapseKey) : t(expandKey)

  return (
    <button
      className={className}
      data-has-unread={hasUnread ? 'true' : undefined}
      type="button"
      aria-label={label}
      aria-pressed={open}
      onClick={onClick}
      title={showTitle ? label : undefined}
    >
      <SidebarToggleIcon open={open} side={side} />
    </button>
  )
}

interface SidebarToggleControlsProps {
  bottomOpen: boolean
  onToggleBottomPanel: () => void
  hasUnreadConversations: boolean
  leftOpen: boolean
  onToggleLeftSidebar: () => void
  onToggleRightSidebar: () => void
  rightOpen: boolean
  t: (key: TranslationKey) => string
}

export interface MainPanelConversationActions {
  conversationId: string
  isPinned: boolean
  onArchive: () => void
  onCommitTitle: (title: string) => void
  onRename: () => void
  onTogglePin: () => void
}

export interface MainPanelProjectCardActions {
  conversationCount: number
  onEditProject: () => void
  onRevealFolder: (folderId: string) => void
  project: AppProject
}

interface MainPanelToolbarProps extends SidebarToggleControlsProps {
  conversationActions?: MainPanelConversationActions
  projectCard?: MainPanelProjectCardActions
  title?: string
}

const TITLE_CONVERSATION_MENU_WIDTH = 224
const TITLE_CONVERSATION_MENU_HEIGHT = 148

function MainPanelConversationTitle({
  conversationActions,
  projectCard,
  t,
  title
}: {
  conversationActions?: MainPanelConversationActions
  projectCard?: MainPanelProjectCardActions
  t: (key: TranslationKey) => string
  title: string
}) {
  const menuRootRef = useRef<HTMLDivElement>(null)
  const editingConversationIdRef = useRef<string | null>(null)
  const titleAtEditStartRef = useRef(title)
  const [menuPosition, setMenuPosition] = useState<ConversationActionsMenuPosition | null>(null)
  const [projectOpen, setProjectOpen] = useState(false)
  const [draft, setDraft] = useState(title)
  const [editingConversationId, setEditingConversationId] = useState<string | null>(null)
  const isMenuOpen = Boolean(menuPosition)
  const isEditing = Boolean(
    conversationActions && editingConversationId === conversationActions.conversationId
  )
  const closeMenu = () => setMenuPosition(null)
  const closeProjectCard = () => setProjectOpen(false)
  useDismissOnOutsidePointer(menuRootRef, isMenuOpen, closeMenu)

  useEffect(() => {
    editingConversationIdRef.current = null
    setEditingConversationId(null)
    setProjectOpen(false)
  }, [conversationActions?.conversationId, projectCard?.project.id])

  const finishEditing = (next: string | null) => {
    const conversationId = editingConversationIdRef.current
    editingConversationIdRef.current = null
    setEditingConversationId(null)
    if (
      next === null ||
      !conversationId ||
      conversationId !== conversationActions?.conversationId
    ) {
      return
    }

    const trimmed = next.trim()
    if (!trimmed || trimmed === titleAtEditStartRef.current) return
    conversationActions.onCommitTitle(trimmed)
  }

  const startEditing = () => {
    if (!conversationActions) return
    closeMenu()
    closeProjectCard()
    titleAtEditStartRef.current = title
    setDraft(title)
    editingConversationIdRef.current = conversationActions.conversationId
    setEditingConversationId(conversationActions.conversationId)
  }

  return (
    <div className="main-panel__title" data-editing={isEditing || undefined}>
      {projectCard ? (
        <MainPanelProjectCard
          conversationCount={projectCard.conversationCount}
          onEditProject={projectCard.onEditProject}
          onOpenChange={(nextOpen) => {
            if (nextOpen) closeMenu()
            setProjectOpen(nextOpen)
          }}
          onRevealFolder={projectCard.onRevealFolder}
          open={projectOpen}
          project={projectCard.project}
          t={t}
        />
      ) : null}
      {isEditing ? (
        <input
          className="main-panel__title-input"
          aria-label={t('conversation.renameTitle')}
          autoFocus
          spellCheck={false}
          value={draft}
          onBlur={(event) => finishEditing(event.currentTarget.value)}
          onChange={(event) => setDraft(event.currentTarget.value)}
          onFocus={(event) => event.currentTarget.select()}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return
            if (event.key === 'Enter') {
              event.preventDefault()
              finishEditing(event.currentTarget.value)
            }
            if (event.key === 'Escape') {
              event.preventDefault()
              finishEditing(null)
            }
          }}
        />
      ) : conversationActions ? (
        <button className="main-panel__title-text" type="button" onClick={startEditing}>
          {title}
        </button>
      ) : (
        <h1 className="main-panel__title-text">{title}</h1>
      )}
      {conversationActions ? (
        <div className="main-panel__title-menu" ref={menuRootRef}>
          <button
            className="main-panel__title-menu-button"
            type="button"
            aria-expanded={isMenuOpen}
            aria-haspopup="menu"
            aria-label={t('sidebar.moreConversationActions')}
            data-open={isMenuOpen || undefined}
            onClick={(event) => {
              if (isMenuOpen) {
                closeMenu()
                return
              }

              closeProjectCard()
              const rect = event.currentTarget.getBoundingClientRect()
              setMenuPosition({
                x: Math.max(
                  12,
                  Math.min(
                    rect.right - TITLE_CONVERSATION_MENU_WIDTH,
                    window.innerWidth - TITLE_CONVERSATION_MENU_WIDTH - 12
                  )
                ),
                y: Math.max(
                  12,
                  Math.min(
                    rect.bottom + 4,
                    window.innerHeight - TITLE_CONVERSATION_MENU_HEIGHT - 12
                  )
                )
              })
            }}
          >
            <MoreHorizontal aria-hidden="true" />
          </button>
          {menuPosition ? (
            <ConversationActionsMenu
              archiveLabel={t('conversation.archiveConversation')}
              isPinned={conversationActions.isPinned}
              menuPosition={menuPosition}
              onArchive={conversationActions.onArchive}
              onClose={closeMenu}
              onRename={conversationActions.onRename}
              onTogglePin={conversationActions.onTogglePin}
              pinLabel={t('conversation.pinConversation')}
              renameLabel={t('conversation.renameConversation')}
              unpinLabel={t('conversation.unpinConversation')}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export function MainPanelToolbar({
  bottomOpen,
  conversationActions,
  onToggleBottomPanel,
  hasUnreadConversations,
  leftOpen,
  onToggleLeftSidebar,
  onToggleRightSidebar,
  projectCard,
  rightOpen,
  t,
  title
}: MainPanelToolbarProps) {
  return (
    <div className="main-panel__toolbar" data-drag-region>
      <PanelToggleButton
        className="panel-toggle panel-toggle--left"
        hasUnread={!leftOpen && hasUnreadConversations}
        onClick={onToggleLeftSidebar}
        open={leftOpen}
        side="left"
        t={t}
      />
      <PanelToggleButton
        className="panel-toggle panel-toggle--bottom"
        onClick={onToggleBottomPanel}
        open={bottomOpen}
        side="bottom"
        t={t}
      />
      <PanelToggleButton
        className="panel-toggle panel-toggle--right"
        onClick={onToggleRightSidebar}
        open={rightOpen}
        side="right"
        t={t}
      />
      {title ? (
        <MainPanelConversationTitle
          conversationActions={conversationActions}
          projectCard={projectCard}
          t={t}
          title={title}
        />
      ) : (
        <MainPanelBrandMark />
      )}
    </div>
  )
}

export function MaximizedSidebarControls({
  bottomOpen,
  onToggleBottomPanel,
  hasUnreadConversations,
  leftOpen,
  onToggleLeftSidebar,
  onToggleRightSidebar,
  rightOpen,
  t
}: SidebarToggleControlsProps) {
  return (
    <>
      <PanelToggleButton
        className="right-sidebar__icon-button"
        hasUnread={!leftOpen && hasUnreadConversations}
        onClick={onToggleLeftSidebar}
        open={leftOpen}
        showTitle
        side="left"
        t={t}
      />
      <PanelToggleButton
        className="right-sidebar__icon-button"
        onClick={onToggleBottomPanel}
        open={bottomOpen}
        showTitle
        side="bottom"
        t={t}
      />
      <PanelToggleButton
        className="right-sidebar__icon-button"
        onClick={onToggleRightSidebar}
        open={rightOpen}
        showTitle
        side="right"
        t={t}
      />
    </>
  )
}
