import type { MenuItemConstructorOptions } from 'electron'
import type { StorageChatConversationRecord } from '@mycopilot/protocol'

export const MENU_BAR_RUNNING_CONVERSATION_REFRESH_MS = 5_000

const ACTIVE_RUN_STATUSES = new Set([
  'queued',
  'running',
  'waiting_for_approval',
  'waiting_for_user_input'
])

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
  loadConversations(): Promise<StorageChatConversationRecord[]>
  openConversation(conversationId: string): void
  quit(): void
}

function isActiveAgentRun(agentRunJson: string | null | undefined): boolean {
  if (!agentRunJson) return false
  try {
    const parsed: unknown = JSON.parse(agentRunJson)
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return false
    const status = 'status' in parsed ? parsed.status : undefined
    return typeof status === 'string' && ACTIVE_RUN_STATUSES.has(status)
  } catch {
    return false
  }
}

function normalizeTitle(title: string, fallback: string): string {
  const normalized = title.replace(/\s+/g, ' ').trim()
  return normalized || fallback
}

/**
 * Conversation records have already removed all child-agent conversations in Core storage. A
 * root remains active while an assistant reply is pending or carries an active run state.
 */
export function selectMenuBarRunningConversations(
  conversations: readonly StorageChatConversationRecord[],
  untitled: string
): MenuBarRunningConversation[] {
  return conversations
    .filter(
      (conversation) =>
        !conversation.archivedAt &&
        conversation.messages.some(
          (message) =>
            message.role === 'assistant' &&
            (message.status === 'pending' || isActiveAgentRun(message.agentRunJson))
        )
    )
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
  private refreshScheduled = false
  private refreshTimer: ReturnType<typeof setInterval> | null = null

  constructor(private readonly dependencies: MenuBarRunningConversationsControllerDependencies) {}

  start(): void {
    this.refresh()
    this.refreshTimer = setInterval(() => this.refresh(), MENU_BAR_RUNNING_CONVERSATION_REFRESH_MS)
  }

  dispose(): void {
    this.disposed = true
    if (this.refreshTimer) clearInterval(this.refreshTimer)
    this.refreshTimer = null
  }

  refresh(): void {
    this.refreshRequested = true
    if (this.refreshInFlight || this.refreshScheduled || this.disposed) return
    this.refreshScheduled = true
    queueMicrotask(() => {
      this.refreshScheduled = false
      if (this.refreshInFlight || this.disposed) return
      this.refreshInFlight = true
      void this.drainRefreshes()
    })
  }

  private async drainRefreshes(): Promise<void> {
    try {
      while (this.refreshRequested && !this.disposed) {
        this.refreshRequested = false
        const records = await this.dependencies.loadConversations()
        if (this.disposed) return
        const conversations = selectMenuBarRunningConversations(
          records,
          this.dependencies.labels.untitled
        )
        const signature = JSON.stringify(conversations)
        if (signature === this.lastSignature) continue
        this.lastSignature = signature
        this.dependencies.applyMenu(
          buildMenuBarRunningConversationsMenu(
            conversations,
            this.dependencies.labels,
            this.dependencies.openConversation,
            this.dependencies.quit
          )
        )
      }
    } catch {
      // Core can be restarting while the app is open. Preserve the last native menu and retry on
      // the next event or interval instead of surfacing an internal error to the user.
    } finally {
      this.refreshInFlight = false
      if (this.refreshRequested && !this.disposed) this.refresh()
    }
  }
}
