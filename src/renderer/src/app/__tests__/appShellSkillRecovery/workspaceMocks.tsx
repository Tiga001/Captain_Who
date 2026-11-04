import type { NotificationOpenRequest } from '@mycopilot/protocol'
import { vi } from 'vitest'
import type { ChatComposerDraft, ChatConversation } from '../../../features/chat/chatTypes'
import type {
  RightSidebarModuleNavigationRequest,
  RightSidebarWorkflowNavigationRequest
} from '../../../features/rightSidebar/rightSidebarTypes'
import { testState } from './state'

vi.mock('../../../components/layout/ResizeHandle', () => ({ ResizeHandle: () => null }))
vi.mock('../../../features/automations/useAutomationAttention', () => ({
  useAutomationAttention: () => ({ unreadCount: 4 })
}))
vi.mock('../../../features/notifications/notificationClient', () => ({
  markNotificationsSeen: testState.markNotificationsSeen,
  onNotificationOpenRequested: (listener: (request: NotificationOpenRequest) => void) => {
    testState.notificationOpenRequestListeners.add(listener)
    return () => testState.notificationOpenRequestListeners.delete(listener)
  }
}))
vi.mock('../../../features/automations/ScheduledPageLayer', () => ({
  ScheduledPageLayer: ({
    externalNavigationRequest,
    initialPreferredDrawerWidth,
    onPreferredDrawerWidthChange,
    openRequest
  }: {
    externalNavigationRequest?: { proceed: () => void; requestKey: number }
    initialPreferredDrawerWidth?: number
    onPreferredDrawerWidthChange?: (width: number) => void
    openRequest?: { automationId: string; runId: string | null }
  }) => (
    <section data-testid="scheduled-page-layer">
      <output data-testid="scheduled-open-automation">{openRequest?.automationId ?? 'none'}</output>
      <output data-testid="scheduled-open-run">{openRequest?.runId ?? 'none'}</output>
      <output data-testid="scheduled-drawer-preferred-width">
        {initialPreferredDrawerWidth ?? 'none'}
      </output>
      <button type="button" onClick={() => onPreferredDrawerWidthChange?.(560)}>
        resize-scheduled-drawer
      </button>
      <output data-testid="external-navigation-request-key">
        {externalNavigationRequest?.requestKey ?? 'none'}
      </output>
      <button type="button" onClick={() => externalNavigationRequest?.proceed()}>
        confirm-external-navigation
      </button>
    </section>
  )
}))
vi.mock('../../shell/sidebar/LeftSidebar', () => ({
  LeftSidebar: ({
    activeConversationId,
    conversations,
    onArchiveConversation,
    onNewConversation,
    onOpenSettings,
    onOpenScheduled,
    onOpenWorkflows,
    onRenameConversation,
    onSelectConversation,
    scheduledAttentionCount,
    scheduledSelected,
    uiPreferences
  }: {
    activeConversationId: string | null
    conversations: ChatConversation[]
    onArchiveConversation: (conversationId: string) => void
    onNewConversation: (projectId?: string | null) => void
    onOpenSettings: () => void
    onOpenScheduled: () => void
    onOpenWorkflows: () => void
    onRenameConversation: (conversationId: string, title: string) => void
    onSelectConversation: (conversationId: string) => void
    scheduledAttentionCount: number
    scheduledSelected: boolean
    uiPreferences: { translucentSidebar: boolean }
  }) => (
    <div>
      <output data-testid="sidebar-translucent">{String(uiPreferences.translucentSidebar)}</output>
      <output data-testid="sidebar-active-conversation">{activeConversationId ?? 'none'}</output>
      <output data-testid="scheduled-selected">{String(scheduledSelected)}</output>
      <output data-testid="scheduled-attention">{scheduledAttentionCount}</output>
      <button type="button" onClick={onOpenScheduled}>
        open-scheduled
      </button>
      <button type="button" onClick={onOpenWorkflows}>
        open-workflows
      </button>
      <button type="button" onClick={onOpenSettings}>
        open-settings
      </button>
      <button type="button" onClick={() => onNewConversation(null)}>
        new-conversation
      </button>
      <button type="button" onClick={() => onNewConversation('project-a')}>
        new-conversation-project-a
      </button>
      {conversations.map((conversation: ChatConversation) => (
        <div key={conversation.id}>
          <button type="button" onClick={() => onSelectConversation(conversation.id)}>
            select-{conversation.id}
          </button>
          <button type="button" onClick={() => onArchiveConversation(conversation.id)}>
            archive-{conversation.id}
          </button>
          <button
            type="button"
            onClick={() => onRenameConversation(conversation.id, `renamed-${conversation.id}`)}
          >
            rename-{conversation.id}
          </button>
          <output data-testid={`title-${conversation.id}`}>{conversation.title}</output>
          <output data-testid={`unread-${conversation.id}`}>
            {conversation.unreadAt ? 'true' : 'false'}
          </output>
          <output data-testid={`archived-${conversation.id}`}>
            {conversation.archivedAt ? 'true' : 'false'}
          </output>
          <output data-testid={`archive-pending-${conversation.id}`}>
            {conversation.pendingArchivedAt === undefined ? 'false' : 'true'}
          </output>
        </div>
      ))}
    </div>
  )
}))
vi.mock('../../../features/rightSidebar/RightSidebar', () => ({
  RightSidebar: ({
    activeConversationId,
    moduleNavigationRequest,
    workflowNavigationRequest
  }: {
    activeConversationId?: string | null
    moduleNavigationRequest?: RightSidebarModuleNavigationRequest | null
    workflowNavigationRequest?: RightSidebarWorkflowNavigationRequest | null
  }) => (
    <>
      <output data-testid="right-sidebar-conversation-id">{activeConversationId ?? 'none'}</output>
      <output data-testid="right-module-request">{JSON.stringify(moduleNavigationRequest)}</output>
      <output data-testid="workflow-route">{JSON.stringify(workflowNavigationRequest)}</output>
    </>
  )
}))
vi.mock('../../../features/bottomPanel/BottomPanel', () => ({
  BottomPanel: ({
    moduleNavigationRequest
  }: {
    moduleNavigationRequest?: RightSidebarModuleNavigationRequest | null
  }) => (
    <output data-testid="bottom-module-request">{JSON.stringify(moduleNavigationRequest)}</output>
  )
}))
vi.mock('../../AppShellSettingsView', () => ({
  AppShellSettingsView: () => <section data-testid="settings-view" />
}))
vi.mock('../../../features/chat/NewConversationPage', () => ({
  NewConversationPage: ({
    draft,
    promptIndex
  }: {
    draft: ChatComposerDraft
    promptIndex: number
  }) => (
    <div>
      <output data-testid="new-conversation-draft">{draft.message}</output>
      <output data-testid="new-conversation-project">{draft.projectId ?? 'none'}</output>
      <output data-testid="new-conversation-prompt-index">{promptIndex}</output>
      <output data-testid="new-conversation-payload">
        {JSON.stringify({
          attachments: draft.attachments,
          message: draft.message,
          modelId: draft.modelId,
          permissionMode: draft.permissionMode,
          projectId: draft.projectId,
          skills: draft.skills
        })}
      </output>
    </div>
  )
}))
