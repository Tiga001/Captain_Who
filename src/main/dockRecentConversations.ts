import type { MenuItemConstructorOptions } from 'electron'
import type { StorageChatConversationMetaRecord } from '@mycopilot/protocol'

export const DOCK_RECENT_CONVERSATION_LIMIT = 3
export const DOCK_RECENT_CONVERSATION_REFRESH_MS = 5_000

export interface DockRecentConversationLabels {
  empty: string
  header: string
  more: string
  untitled: string
}

export interface DockRecentConversation {
  id: string
  title: string
  updatedAt: number
}

export interface DockRecentConversationsControllerDependencies {
  applyMenu(template: MenuItemConstructorOptions[]): void
  labels: DockRecentConversationLabels
  loadConversationMetas(): Promise<StorageChatConversationMetaRecord[]>
  openConversation(conversationId: string): void
}

function normalizeTitle(title: string, fallback: string): string {
  const normalized = title.replace(/\s+/g, ' ').trim()
  return normalized || fallback
}

/**
 * The storage endpoint is deliberately limited to user-facing root conversations. Keep this
 * projection small and pure so the native menu cannot accidentally expose agent internals.
 */
export function selectDockRecentConversations(
  records: readonly StorageChatConversationMetaRecord[],
  untitled: string
): DockRecentConversation[] {
  return records
    .filter((record) => !record.archivedAt && record.id.trim().length > 0)
    .map((record) => ({
      id: record.id,
      title: normalizeTitle(record.title, untitled),
      updatedAt: record.updatedAt
    }))
    .sort((left, right) => right.updatedAt - left.updatedAt || left.id.localeCompare(right.id))
}

export function buildDockRecentConversationsMenu(
  conversations: readonly DockRecentConversation[],
  labels: DockRecentConversationLabels,
  openConversation: (conversationId: string) => void
): MenuItemConstructorOptions[] {
  const conversationItems = conversations.map((conversation) => ({
    label: conversation.title,
    click: () => openConversation(conversation.id)
  }))
  const visibleItems = conversationItems.slice(0, DOCK_RECENT_CONVERSATION_LIMIT)

  return [
    { label: labels.header, enabled: false },
    { type: 'separator' },
    ...(visibleItems.length > 0 ? visibleItems : [{ label: labels.empty, enabled: false }]),
    { type: 'separator' },
    {
      label: labels.more,
      enabled: conversationItems.length > 0,
      submenu: conversationItems
    }
  ]
}

/**
 * A short metadata-only refresh keeps the Dock menu current even when a conversation changes
 * through a background workflow that does not travel through the Renderer's storage IPC.
 */
export class DockRecentConversationsController {
  private disposed = false
  private lastSignature: string | null = null
  private refreshTimer: ReturnType<typeof setInterval> | null = null

  constructor(private readonly dependencies: DockRecentConversationsControllerDependencies) {}

  start(): void {
    this.refresh()
    this.refreshTimer = setInterval(() => this.refresh(), DOCK_RECENT_CONVERSATION_REFRESH_MS)
  }

  dispose(): void {
    this.disposed = true
    if (this.refreshTimer) clearInterval(this.refreshTimer)
    this.refreshTimer = null
  }

  refresh(): void {
    void this.refreshNow().catch(() => {
      // Core startup and shutdown can briefly make the metadata endpoint unavailable. The next
      // refresh retries; retaining the last menu is preferable to replacing it with an error.
    })
  }

  async refreshNow(): Promise<void> {
    const records = await this.dependencies.loadConversationMetas()
    if (this.disposed) return

    const conversations = selectDockRecentConversations(records, this.dependencies.labels.untitled)
    const signature = JSON.stringify(conversations)
    if (signature === this.lastSignature) return

    this.lastSignature = signature
    this.dependencies.applyMenu(
      buildDockRecentConversationsMenu(
        conversations,
        this.dependencies.labels,
        this.dependencies.openConversation
      )
    )
  }
}
