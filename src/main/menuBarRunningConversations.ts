import type { MenuItemConstructorOptions } from 'electron'
import type { AgentEvent, StorageRunningConversationSummary } from '@mycopilot/protocol'

export const MENU_BAR_RUNNING_CONVERSATION_REFRESH_MS = 5_000
export const MENU_BAR_RUNNING_CONVERSATION_COALESCE_MS = 100
export const MENU_BAR_RUNNING_CONVERSATION_MAX_DEFER_MS = 500

/** Text, command output and tool previews cannot change active membership. */
export function agentEventChangesRunningConversations(event: Pick<AgentEvent, 'type'>): boolean {
  return (
    event.type === 'started' ||
    event.type === 'state' ||
    event.type === 'approval_required' ||
    event.type === 'done' ||
    event.type === 'error'
  )
}

export interface MenuBarRunningConversation {
  id: string
  title: string
  updatedAt: number
}

export interface MenuBarRunningConversationLabels {
  appName: string
  empty: string
  quit: string
  running: string
  untitled: string
}

export interface MenuBarRunningConversationsControllerDependencies {
  applyMenu(template: MenuItemConstructorOptions[]): void
  labels: MenuBarRunningConversationLabels
  loadRunningConversationSummaries(): Promise<StorageRunningConversationSummary[]>
  openConversation(conversationId: string): void
  quit(): void
}

function normalizeTitle(title: string, fallback: string): string {
  const normalized = title.replace(/\s+/g, ' ').trim()
  return normalized || fallback
}

/**
 * Core owns active membership, including root/archived filtering and durable terminal state.
 */
export function selectMenuBarRunningConversations(
  conversations: readonly StorageRunningConversationSummary[],
  untitled: string
): MenuBarRunningConversation[] {
  return conversations
    .map((conversation) => ({
      id: conversation.id,
      title: normalizeTitle(conversation.title, untitled),
      updatedAt: conversation.updatedAt
    }))
    .sort((left, right) => right.updatedAt - left.updatedAt || left.id.localeCompare(right.id))
}

export function buildMenuBarRunningConversationsMenu(
  conversations: readonly MenuBarRunningConversation[],
  labels: MenuBarRunningConversationLabels,
  openConversation: (conversationId: string) => void,
  quit: () => void
): MenuItemConstructorOptions[] {
  const conversationItems = conversations.map((conversation) => ({
    label: conversation.title,
    click: () => openConversation(conversation.id)
  }))
  return [
    { label: labels.appName, enabled: false },
    { type: 'separator' },
    { label: labels.running, enabled: false },
    ...(conversationItems.length > 0
      ? conversationItems
      : [{ label: labels.empty, enabled: false }]),
    { type: 'separator' },
    { label: labels.quit, click: quit }
  ]
}

export class MenuBarRunningConversationsController {
  private disposed = false
  private lastSignature: string | null = null
  private refreshInFlight = false
  private refreshRequested = false
  private invalidatedSince: number | null = null
  private consecutiveFailures = 0
  private scheduledRefresh: ReturnType<typeof setTimeout> | null = null
  private refreshTimer: ReturnType<typeof setInterval> | null = null

  constructor(private readonly dependencies: MenuBarRunningConversationsControllerDependencies) {}

  start(): void {
    if (this.disposed || this.refreshTimer) return
    this.refresh()
    this.refreshTimer = setInterval(() => this.refresh(), MENU_BAR_RUNNING_CONVERSATION_REFRESH_MS)
  }

  dispose(): void {
    this.disposed = true
    if (this.refreshTimer) clearInterval(this.refreshTimer)
    this.refreshTimer = null
    if (this.scheduledRefresh) clearTimeout(this.scheduledRefresh)
    this.scheduledRefresh = null
  }

  refresh(): void {
    if (this.disposed) return
    this.refreshRequested = true
    this.invalidatedSince ??= Date.now()
    if (this.refreshInFlight || this.scheduledRefresh) return
    // Fixed from the first invalidation: a sustained stream cannot extend the deadline.
    this.scheduledRefresh = setTimeout(
      () => {
        this.scheduledRefresh = null
        if (this.refreshInFlight || this.disposed) return
        this.refreshInFlight = true
        void this.drainRefreshes()
      },
      this.consecutiveFailures === 0
        ? MENU_BAR_RUNNING_CONVERSATION_COALESCE_MS
        : Math.min(
            MENU_BAR_RUNNING_CONVERSATION_REFRESH_MS,
            MENU_BAR_RUNNING_CONVERSATION_COALESCE_MS * 2 ** Math.min(this.consecutiveFailures, 6)
          )
    )
  }

  private async drainRefreshes(): Promise<void> {
    try {
      this.refreshRequested = false
      const records = await this.dependencies.loadRunningConversationSummaries()
      this.consecutiveFailures = 0
      if (this.disposed) return
      // Prefer a fresh response, but do not starve native menus while several runs change state.
      // At the deadline publish the latest available snapshot and continue the serial requery.
      if (
        this.refreshRequested &&
        this.invalidatedSince !== null &&
        Date.now() - this.invalidatedSince < MENU_BAR_RUNNING_CONVERSATION_MAX_DEFER_MS
      )
        return
      this.invalidatedSince = this.refreshRequested ? Date.now() : null
      const conversations = selectMenuBarRunningConversations(
        records,
        this.dependencies.labels.untitled
      )
      const signature = JSON.stringify(conversations.map(({ id, title }) => ({ id, title })))
      if (signature === this.lastSignature) return
      this.lastSignature = signature
      this.dependencies.applyMenu(
        buildMenuBarRunningConversationsMenu(
          conversations,
          this.dependencies.labels,
          this.dependencies.openConversation,
          this.dependencies.quit
        )
      )
    } catch {
      // Core can be restarting while the app is open. Preserve the last native menu and retry on
      // the next event or interval instead of surfacing an internal error to the user.
      this.consecutiveFailures += 1
    } finally {
      this.refreshInFlight = false
      if (this.refreshRequested && !this.disposed) this.refresh()
    }
  }
}
