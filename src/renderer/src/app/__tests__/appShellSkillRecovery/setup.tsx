import type {
  AgentConversationTurnRewriteInput,
  AgentEvent,
  AgentProviderTransitionNotification
} from '@mycopilot/protocol'
import { afterEach, beforeEach, expect, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { testState } from './state'
import { commitSuccessfulRewrite } from './turnFixtures'
import { completedProviderTransition } from './eventFixtures'
import { storedConversation } from './conversationFixtures'

// Keep mock registration in the original order and complete it before loading AppShell.
await import('./providerMocks')
await import('./hostMocks')
await import('./workspaceMocks')
await import('./conversationPageMock')

export const [
  { AppShell },
  { createComposerDraft, createConversationTitle },
  { defaultUiPreferences }
] = await Promise.all([
  import('../../AppShell'),
  import('../../chatMessageFactory'),
  import('../../../features/storage/storageClient')
])

beforeEach(() => {
  testState.openEditProjectDialog.mockReset().mockResolvedValue('cancelled')
  testState.deleteProject.mockReset().mockResolvedValue(undefined)
  testState.canOpenBottomPanel = true
  testState.gitStatus = 'unavailable'
  testState.openBottomPanel.mockReset()
  testState.openRightSidebar.mockReset()
  testState.setRightSidebarMaximized.mockReset()
  testState.rightOpen = false
  testState.rightMaximized = false
  testState.automationEventListeners.clear()
  testState.automationOpenRequestListeners.clear()
  testState.automationResyncListeners.clear()
  testState.notificationOpenRequestListeners.clear()
  testState.markNotificationsSeen.mockReset().mockResolvedValue(undefined)
  testState.cancelAgentRun.mockReset().mockResolvedValue(true)
  testState.collaborationRootIds.length = 0
  testState.deleteChatMessages.mockReset().mockResolvedValue(undefined)
  testState.forkConversation.mockReset()
  testState.getManualContextCompactionStatus.mockReset().mockResolvedValue({ operations: [] })
  testState.getContextWindowSnapshot.mockReset().mockResolvedValue({ modelConfigId: 'model-1' })
  testState.getProviderTransitionStatus.mockReset().mockResolvedValue({ operations: [] })
  testState.getAgentCommandSession.mockReset()
  testState.listAgentCommandSessions.mockReset().mockResolvedValue({ sessions: [] })
  testState.listPendingAgentActions.mockReset().mockResolvedValue([])
  testState.loadComposerDrafts.mockReset().mockResolvedValue({
    'conversation-a': createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
  })
  const stored = storedConversation()
  testState.persistedConversations.clear()
  testState.persistedConversations.set(stored.id, stored)
  testState.loadConversation
    .mockReset()
    .mockImplementation(async (conversationId: string) =>
      testState.persistedConversations.get(conversationId)
    )
  testState.loadConversationMetas.mockReset().mockImplementation(async () =>
    [...testState.persistedConversations.values()].map((conversation) => ({
      ...conversation,
      messages: [],
      messagesLoaded: false
    }))
  )
  testState.loadInputAttachments.mockReset().mockResolvedValue([])
  testState.loadUiPreferences.mockReset().mockResolvedValue({
    ...defaultUiPreferences(),
    showContextWindowUsage: false
  })
  testState.saveChatMessageState.mockReset().mockResolvedValue(undefined)
  testState.approveAgentAction.mockReset()
  testState.rejectAgentAction.mockReset()
  testState.cancelAgentAction.mockReset()
  testState.agentEventListeners.clear()
  testState.providerTransitionListeners.clear()
  testState.providerTransitionSequence = 0
  testState.onAgentEvent.mockReset().mockImplementation((listener: (event: AgentEvent) => void) => {
    testState.agentEventListeners.add(listener)
    return () => testState.agentEventListeners.delete(listener)
  })
  testState.onProviderTransition
    .mockReset()
    .mockImplementation((listener: (event: AgentProviderTransitionNotification) => void) => {
      testState.providerTransitionListeners.add(listener)
      return () => testState.providerTransitionListeners.delete(listener)
    })
  testState.preflightProviderTransition
    .mockReset()
    .mockImplementation(
      async ({
        conversationId,
        targetModelId
      }: {
        conversationId: string
        targetModelId: string
      }) => ({
        conversationId,
        targetModelId,
        decision: 'compatible' as const,
        reason: 'same_protocol' as const,
        operationId: `provider-transition-${testState.providerTransitionSequence + 1}`,
        transitionToken: `token-${targetModelId}`
      })
    )
  testState.saveComposerDraft.mockReset().mockResolvedValue(undefined)
  testState.saveComposerDraftMessage.mockReset().mockResolvedValue(true)
  testState.saveConversationMeta
    .mockReset()
    .mockImplementation(async (conversation: ChatConversation) => {
      const storedConversation = testState.persistedConversations.get(conversation.id)
      if (storedConversation && conversation.updatedAt < storedConversation.updatedAt) {
        return
      }
      testState.persistedConversations.set(conversation.id, {
        ...storedConversation,
        ...conversation,
        archivedAt: conversation.pendingArchivedAt ?? conversation.archivedAt,
        pendingArchivedAt: undefined,
        unreadAt: conversation.pendingArchivedAt === undefined ? conversation.unreadAt : null
      })
    })
  testState.showToast.mockReset()
  testState.rewriteConversationTurn
    .mockReset()
    .mockImplementation(async (input: AgentConversationTurnRewriteInput) =>
      commitSuccessfulRewrite(input)
    )
  testState.startConversationTurn.mockReset()
  testState.continueConversationTurn.mockReset()
  testState.startProviderTransition
    .mockReset()
    .mockImplementation(async ({ targetModelId }: { targetModelId: string }) =>
      completedProviderTransition(targetModelId)
    )
  testState.steerAgentRun.mockReset().mockResolvedValue({
    guidanceId: 'guidance-1',
    status: 'queued'
  })
  testState.upsertChatMessages.mockReset().mockResolvedValue(undefined)
})

afterEach(() => {
  vi.useRealTimers()
})

export async function renderSelectedConversation() {
  const screen = await render(<AppShell />)
  const selectConversation = screen.getByRole('button', {
    name: 'select-conversation-a',
    exact: true
  })
  await expect.element(selectConversation).toBeVisible()
  await selectConversation.click()
  await expect.element(screen.getByRole('button', { name: 'submit-with-skill' })).toBeVisible()
  return screen
}
