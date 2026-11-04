import type { AutomationEvent, AutomationOpenRequest, AutomationResync } from '@mycopilot/protocol'
import { vi } from 'vitest'
import { testState } from './state'

vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    storage: { onPredictionPreferenceChanged: testState.onPredictionPreferenceChanged },
    agent: {
      onPromptPreferencesChanged: vi.fn(() => () => undefined),
      onCollaborationSettingsChanged: vi.fn(() => () => undefined),
      requestWorkflows: vi.fn(async () => ({ ok: true, value: { records: [], instances: [] } }))
    },
    app: {
      getWindowState: vi.fn().mockResolvedValue({ isFullScreen: false, isMaximized: false }),
      onWindowStateChange: vi.fn(() => () => undefined)
    },
    automations: {
      onEvent: vi.fn((listener: (event: AutomationEvent) => void) => {
        testState.automationEventListeners.add(listener)
        return () => testState.automationEventListeners.delete(listener)
      }),
      onOpenRequested: vi.fn((listener: (request: AutomationOpenRequest) => void) => {
        testState.automationOpenRequestListeners.add(listener)
        return () => testState.automationOpenRequestListeners.delete(listener)
      }),
      onResync: vi.fn((listener: (event: AutomationResync) => void) => {
        testState.automationResyncListeners.add(listener)
        return () => testState.automationResyncListeners.delete(listener)
      })
    }
  }
}))

vi.mock('../../../features/gitReview/useGitRepositoryCapability', () => ({
  useGitRepositoryCapability: (id?: string, path?: string) => ({
    contextKey: JSON.stringify([id ?? null, path ?? null]),
    status: testState.gitStatus
  })
}))

// This suite is the legacy/no-child AppShell baseline. Collaboration is covered by its own
// root-scoped integration tests and must not alter the old single-Agent fixture.
vi.mock('../../../features/agentCollaboration/useCollaborationStore', () => ({
  useOptionalCollaborationStore: (rootConversationId: string | null) => {
    testState.collaborationRootIds.push(rootConversationId)
    return null
  }
}))

vi.mock('../../../features/agent/agentClient', () => ({
  approveAgentAction: testState.approveAgentAction,
  cancelAgentAction: testState.cancelAgentAction,
  cancelAgentRun: testState.cancelAgentRun,
  getContextWindowSnapshot: testState.getContextWindowSnapshot,
  getManualContextCompactionStatus: testState.getManualContextCompactionStatus,
  onManualContextCompaction: vi.fn(() => () => undefined),
  startManualContextCompaction: vi.fn(),
  cancelManualContextCompaction: vi.fn(),
  getProviderTransitionStatus: testState.getProviderTransitionStatus,
  getAgentCommandSession: testState.getAgentCommandSession,
  getAgentFileChangeDiff: vi.fn(),
  getAgentFileChangeHistoryDiff: vi.fn(),
  listAgentCommandSessions: testState.listAgentCommandSessions,
  listPendingAgentActions: testState.listPendingAgentActions,
  onAgentEvent: testState.onAgentEvent,
  onProviderTransition: testState.onProviderTransition,
  preflightProviderTransition: testState.preflightProviderTransition,
  rejectAgentAction: testState.rejectAgentAction,
  rewriteConversationTurn: testState.rewriteConversationTurn,
  startConversationTurn: testState.startConversationTurn,
  continueConversationTurn: testState.continueConversationTurn,
  startProviderTransition: testState.startProviderTransition,
  steerAgentRun: testState.steerAgentRun
}))

vi.mock('../../../features/storage/storageClient', async () => {
  // Browser support modules have URL-shaped importers. A Vite root-relative path keeps
  // the partial mock's actual module identical to AppShell's storageClient import.
  const original = await vi.importActual<typeof import('../../../features/storage/storageClient')>(
    '/src/renderer/src/features/storage/storageClient.ts'
  )
  return {
    ...original,
    deleteChatMessages: testState.deleteChatMessages,
    forkConversation: testState.forkConversation,
    loadComposerDrafts: testState.loadComposerDrafts,
    loadConversation: testState.loadConversation,
    loadConversationMetas: testState.loadConversationMetas,
    loadInputAttachments: testState.loadInputAttachments,
    loadUiPreferences: testState.loadUiPreferences,
    saveChatMessageState: testState.saveChatMessageState,
    saveComposerDraft: testState.saveComposerDraft,
    saveComposerDraftMessage: testState.saveComposerDraftMessage,
    saveConversationMeta: testState.saveConversationMeta,
    saveUiPreferences: vi.fn(),
    upsertChatMessages: testState.upsertChatMessages
  }
})
