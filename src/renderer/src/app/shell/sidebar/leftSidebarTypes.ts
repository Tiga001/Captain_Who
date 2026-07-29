import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../../../features/storage/storageClient'

export type SidebarSectionScope = 'projects' | 'conversations'
export type SidebarSectionSubmenu = 'organize' | 'sort'
export type BulkArchiveScope = 'projects' | 'root'
export type ProjectDragPosition = 'before' | 'after'
export type ConversationListStage = 'collapsed' | 'preview' | 'expanded'
export type SidebarMenuPosition = { left: number; top: number }

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
  onMarkConversationUnread: (conversationId: string) => void
  onNewConversation: (projectId?: string | null) => void
  onOpenSettings: () => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  onRenameConversation: (conversationId: string, title: string) => void
  onRenameProject: (projectId: string, name: string) => void
  onSelectConversation: (conversationId: string, messageId?: string | null) => void
  onShowProjectInFolder: (projectId: string) => void
  onTogglePinConversation: (conversationId: string) => void
  onTogglePinProject: (projectId: string) => void
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  projects: AppProject[]
  uiPreferences: UiPreferencesSnapshot
}
