import type {
  AgentEvent,
  AgentProviderTransitionNotification,
  AutomationEvent,
  AutomationOpenRequest,
  AutomationResync,
  NotificationOpenRequest
} from '@mycopilot/protocol'
import { vi } from 'vitest'
import type { ChatConversation } from '../../../features/chat/chatTypes'

// Vitest browser isolation creates a fresh module graph, including these mocks, for each test file.
const testState = vi.hoisted(() => ({
  canOpenBottomPanel: true,
  gitStatus: 'unavailable' as 'available' | 'checking' | 'unavailable',
  openBottomPanel: vi.fn(),
  openRightSidebar: vi.fn(),
  setRightSidebarMaximized: vi.fn(),
  rightOpen: false,
  rightMaximized: false,
  automationEventListeners: new Set<(event: AutomationEvent) => void>(),
  automationOpenRequestListeners: new Set<(request: AutomationOpenRequest) => void>(),
  automationResyncListeners: new Set<(event: AutomationResync) => void>(),
  notificationOpenRequestListeners: new Set<(request: NotificationOpenRequest) => void>(),
  markNotificationsSeen: vi.fn(async () => undefined),
  cancelAgentRun: vi.fn(),
  approveAgentAction: vi.fn(),
  rejectAgentAction: vi.fn(),
  cancelAgentAction: vi.fn(),
  collaborationRootIds: [] as Array<string | null>,
  deleteChatMessages: vi.fn(),
  forkConversation: vi.fn(),
  getContextWindowSnapshot: vi.fn(),
  getManualContextCompactionStatus: vi.fn(),
  getProviderTransitionStatus: vi.fn(),
  getAgentCommandSession: vi.fn(),
  listAgentCommandSessions: vi.fn(),
  listPendingAgentActions: vi.fn(),
  loadComposerDrafts: vi.fn(),
  loadConversation: vi.fn(),
  loadConversationMetas: vi.fn(),
  loadInputAttachments: vi.fn(),
  loadUiPreferences: vi.fn(),
  onPredictionPreferenceChanged: vi.fn(),
  onAgentEvent: vi.fn(),
  onProviderTransition: vi.fn(),
  persistedConversations: new Map<string, ChatConversation>(),
  agentEventListeners: new Set<(event: AgentEvent) => void>(),
  providerTransitionListeners: new Set<(event: AgentProviderTransitionNotification) => void>(),
  providerTransitionSequence: 0,
  preflightProviderTransition: vi.fn(),
  saveChatMessageState: vi.fn(),
  saveComposerDraft: vi.fn(),
  saveComposerDraftMessage: vi.fn(),
  saveConversationMeta: vi.fn(),
  showToast: vi.fn(),
  rewriteConversationTurn: vi.fn(),
  openEditProjectDialog: vi.fn(),
  deleteProject: vi.fn(),
  startConversationTurn: vi.fn(),
  continueConversationTurn: vi.fn(),
  startProviderTransition: vi.fn(),
  steerAgentRun: vi.fn(),
  upsertChatMessages: vi.fn(),
  enabledModels: [
    {
      id: 'model-1',
      displayName: 'Model One',
      supportsImage: true,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    },
    {
      id: 'model-2',
      displayName: 'Model Two',
      supportsImage: true,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    }
  ],
  projects: [
    {
      id: 'project-a',
      name: 'Project A',
      folders: [
        {
          id: 'project-a-primary',
          path: '/workspace/a',
          alias: 'a',
          role: 'primary' as const,
          sortOrder: 0,
          createdAt: 1
        }
      ],
      createdAt: 1
    }
  ]
}))

const frontendConfig = vi.hoisted(() => ({ t: (key: string) => key }))

export { testState, frontendConfig }
