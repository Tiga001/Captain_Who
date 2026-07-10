// Small constants, queue payload types, and panel toggle controls for AppShell.
import type { CSSProperties } from 'react'
import type { TranslationKey } from '../config/frontendTranslations'
import type { ChatMessage } from '../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { getTranslucentSidebarOpacityPercent } from '../features/storage/storageClient'
import { isMacOS } from '../lib/platform'

export const SUPPORTS_NATIVE_FONT_SMOOTHING = isMacOS()
export const DEFAULT_AGENT_MAX_TOKENS = 30000
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
  uiPreferences: UiPreferencesSnapshot
): CSSProperties {
  return {
    '--left-panel-width': `${leftOpen ? leftWidth : 0}px`,
    '--right-panel-width': `${rightOpen ? rightWidth : 0}px`,
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

type SidebarToggleSide = 'left' | 'right'

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
  const collapseKey = side === 'left' ? 'app.collapseLeftSidebar' : 'app.collapseRightSidebar'
  const expandKey = side === 'left' ? 'app.expandLeftSidebar' : 'app.expandRightSidebar'
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
  hasUnreadConversations: boolean
  leftOpen: boolean
  onToggleLeftSidebar: () => void
  onToggleRightSidebar: () => void
  rightOpen: boolean
  t: (key: TranslationKey) => string
}

export function MainPanelToolbar({
  hasUnreadConversations,
  leftOpen,
  onToggleLeftSidebar,
  onToggleRightSidebar,
  rightOpen,
  t
}: SidebarToggleControlsProps) {
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
        className="panel-toggle panel-toggle--right"
        onClick={onToggleRightSidebar}
        open={rightOpen}
        side="right"
        t={t}
      />
    </div>
  )
}

export function MaximizedSidebarControls({
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
        onClick={onToggleRightSidebar}
        open={rightOpen}
        showTitle
        side="right"
        t={t}
      />
    </>
  )
}
