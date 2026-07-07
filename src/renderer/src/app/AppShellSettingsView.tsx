// Settings view adapter for AppShell conversation and preference callbacks.
import type { Dispatch, SetStateAction } from 'react'
import type { AppProject } from '../config/projectConfig'
import { SettingsPage } from '../features/settings/SettingsPage'
import type { SettingsPageId } from '../features/settings/SettingsPage'
import type { ChatConversation } from '../features/chat/chatTypes'
import type { UiPreferencesSnapshot } from '../features/storage/storageClient'
import { deleteStoredConversation } from '../features/storage/storageClient'

interface AppShellSettingsViewProps {
  conversations: ChatConversation[]
  initialPage: SettingsPageId
  onBack: () => void
  onConversationPatch: (conversationId: string, patch: Partial<ChatConversation>) => void
  onConversationsChange: Dispatch<SetStateAction<ChatConversation[]>>
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  projects: AppProject[]
  uiPreferences: UiPreferencesSnapshot
}

export function AppShellSettingsView({
  conversations,
  initialPage,
  onBack,
  onConversationPatch,
  onConversationsChange,
  onUiPreferencesChange,
  projects,
  uiPreferences
}: AppShellSettingsViewProps) {
  return (
    <SettingsPage
      conversations={conversations}
      initialPage={initialPage}
      projects={projects}
      uiPreferences={uiPreferences}
      onBack={onBack}
      onDeleteAllArchivedConversations={() =>
        onConversationsChange((currentConversations) => {
          currentConversations
            .filter((conversation) => conversation.archivedAt)
            .forEach((conversation) => void deleteStoredConversation(conversation.id))
          return currentConversations.filter((conversation) => !conversation.archivedAt)
        })
      }
      onDeleteConversation={(conversationId) => {
        onConversationsChange((currentConversations) =>
          currentConversations.filter((conversation) => conversation.id !== conversationId)
        )
        void deleteStoredConversation(conversationId)
      }}
      onUnarchiveConversation={(conversationId) =>
        onConversationPatch(conversationId, { archivedAt: null })
      }
      onUiPreferencesChange={onUiPreferencesChange}
    />
  )
}
