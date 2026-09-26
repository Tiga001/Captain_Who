import type { AppProject } from '../../../config/projectConfig'
import type { ProjectEditDialogResult } from '../../../config/ProjectSettingsProvider'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../../../features/storage/storageClient'

export type SidebarSectionScope = 'projects' | 'conversations'
export type SidebarSectionSubmenu = 'organize' | 'sort'
export type BulkArchiveScope = 'projects' | 'root'
export type ProjectDragPosition = 'before' | 'after'
export type ConversationListStage = 'collapsed' | 'preview' | 'expanded'
export type SidebarMenuPosition = { left: number; top: number }

export interface SidebarConversation {
  archivedAt?: number | null
  createdAt: number
  id: string
  isPending?: boolean
  isWaitingForApproval?: boolean
  messages?: ChatConversation['messages']
  workflow?: { id: string; name: string; color: string }
  modelId?: string | null
  pinnedAt?: number | null
  projectId: string | null
  title: string
  unreadAt?: number | null
  updatedAt: number
}

export interface ProjectDragTarget {
  projectId: string
  position: ProjectDragPosition
}

export interface ProjectPointerDragState {
  hasMoved: boolean
  pointerId: number
  projectId: string
  sectionProjectIds: string[]
  startY: number
}

export interface LeftSidebarProps {
  activeConversationId: string | null
  conversations: ChatConversation[]
  onArchiveAllProjectConversations: () => void
  onArchiveAllRootConversations: () => void
  onArchiveConversation: (conversationId: string) => void
  onArchiveProjectConversations: (projectId: string) => void
  /** Opens the project editor; `remove-requested` hands off to the sidebar's removal confirm. */
  onEditProject: (projectId: string) => Promise<ProjectEditDialogResult>
  onMarkConversationUnread: (conversationId: string) => void
  onNewConversation: (projectId?: string | null) => void
  onNewProject: () => Promise<AppProject | null>
  onOpenSettings: () => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  onRequestRenameConversation?: (conversationId: string) => void
  onRenameConversation: (conversationId: string, title: string) => void
  onOpenWorkflows?: () => void
  workflowsSelected?: boolean
  workflowMemberships?: Readonly<Record<string, { id: string; name: string; color: string }>>
  onOpenScheduled: () => void
  onSelectConversation: (conversationId: string, messageId?: string | null) => void
  onShowProjectInFolder: (projectId: string) => void
  onTogglePinConversation: (conversationId: string) => void
  onTogglePinProject: (projectId: string) => void
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  projects: AppProject[]
  scheduledAttentionCount: number
  scheduledSelected: boolean
  uiPreferences: UiPreferencesSnapshot
}
