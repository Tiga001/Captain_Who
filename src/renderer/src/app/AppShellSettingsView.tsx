import type { Dispatch, SetStateAction } from 'react'
import type { AppProject } from '../config/projectConfig'
import { SettingsPage } from '../features/settings/SettingsPage'
import type { SettingsPageId } from '../features/settings/SettingsPage'
import type { BrowserAutomationView } from '../features/mcp/BrowserAutomationSettingsPage'
import type { ChatConversation } from '../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { deleteStoredConversation } from '../features/storage/storageClient'

interface AppShellSettingsViewProps {
  conversations: ChatConversation[]
  initialPage: SettingsPageId
  initialBrowserView?: BrowserAutomationView
  initialProjectId?: string | null
  onBack: () => void
  onBeforeConversationDelete: (conversationId: string) => Promise<void>
  onConversationPatch: (conversationId: string, patch: Partial<ChatConversation>) => void
  onConversationsChange: Dispatch<SetStateAction<ChatConversation[]>>
  onRemoveProject: (projectId: string) => Promise<boolean>
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  projects: AppProject[]
  uiPreferences: UiPreferencesSnapshot
}

export function AppShellSettingsView({
  conversations,
  initialPage,
  initialBrowserView,
  initialProjectId,
  onBack,
  onBeforeConversationDelete,
  onConversationPatch,
  onConversationsChange,
  onRemoveProject,
  onUiPreferencesChange,
  projects,
  uiPreferences
}: AppShellSettingsViewProps) {
  return (
    <SettingsPage
      conversations={conversations}
      initialPage={initialPage}
      initialBrowserView={initialBrowserView}
      initialProjectId={initialProjectId}
      projects={projects}
      uiPreferences={uiPreferences}
      onBack={onBack}
      onDeleteArchivedConversations={(conversationIds) =>
        onConversationsChange((currentConversations) => {
          const conversationIdSet = new Set(conversationIds)
          const deletedConversationIds = new Set<string>()
          const nextConversations = currentConversations.filter((conversation) => {
            if (!conversationIdSet.has(conversation.id)) {
              return true
            }
            deletedConversationIds.add(conversation.id)
            return false
          })

          deletedConversationIds.forEach((conversationId) => {
            void onBeforeConversationDelete(conversationId)
              .then(() => deleteStoredConversation(conversationId))
              .catch((error) => console.error('Failed to delete stored conversation', error))
          })
          return nextConversations
        })
      }
      onDeleteConversation={(conversationId) => {
        onConversationsChange((currentConversations) =>
          currentConversations.filter((conversation) => conversation.id !== conversationId)
        )
        void onBeforeConversationDelete(conversationId)
          .then(() => deleteStoredConversation(conversationId))
          .catch((error) => console.error('Failed to delete stored conversation', error))
      }}
      onRemoveProject={onRemoveProject}
      onUnarchiveConversation={(conversationId) =>
        onConversationPatch(conversationId, { archivedAt: null })
      }
      onUiPreferencesChange={onUiPreferencesChange}
    />
  )
}
