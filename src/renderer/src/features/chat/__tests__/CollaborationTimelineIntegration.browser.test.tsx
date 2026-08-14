import type { CSSProperties } from 'react'
import { expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicLightTheme } from '../../../config/themes/classic'
import { ChatMessageList, ConversationSurface } from '../ChatConversationPage'
import type { ChatConversation } from '../chatTypes'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import '../../../styles/global.css'

const translations: Record<string, string> = {
  'chat.copyMessage': 'Copy message',
  'chat.messageActions': 'Message actions',
  'collaboration.activity.openAgentActivity': 'View sub-agent {name}: {status}',
  'collaboration.activity.status.completed': 'Completed',
  'collaboration.activity.status.failed': 'Failed',
  'collaboration.activity.status.interrupted': 'Interrupted',
  'collaboration.activity.status.started': 'Started working',
  'collaboration.activity.status.updated': 'Updated',
  'collaboration.activity.status.waitingApproval': 'Waiting for approval'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../components/ChatComposer', () => ({
  ChatComposer: () => <div aria-label="Message composer" className="chat-composer" />
}))

function conversation(): ChatConversation {
  return {
    id: 'root-conversation',
    projectId: 'project-a',
    modelId: 'model-root',
    title: 'Root conversation',
    messages: [
      {
        id: 'root-user-1',
        role: 'user',
        content: 'Delegate the review.',
        createdAt: 1_000,
        status: 'sent'
      },
      {
        id: 'root-assistant-1',
        role: 'assistant',
        content: 'I delegated the review.',
        createdAt: 2_000,
        status: 'sent'
      },
      {
        id: 'root-user-2',
        role: 'user',
        content: 'Continue.',
        createdAt: 5_000,
        status: 'sent'
      }
    ],
    messagesLoaded: true,
    createdAt: 1_000,
    updatedAt: 5_000,
    archivedAt: null,
    unreadAt: null
  }
}

function activity(
  activityId: string,
  semantic: CollaborationTimelineActivity['semantic'],
  sequence: number,
  occurredAt: number,
  rootAnchorMessageId: string | null
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId: 'agent-reviewer',
    occurredAt,
    rootAnchorMessageId,
    runId: `run-${sequence}`,
    semantic,
    sequence,
    taskNameSnapshot: 'Reviewer',
    turnId: `turn-${sequence}`
  }
}

it('merges typed durable activity into the one message timeline without a bottom summary panel', async () => {
  const onOpenAgent = vi.fn()
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[
        activity('event-unanchored', 'updated', 2, 3_000, null),
        activity('event-anchored', 'started', 1, 9_000, 'root-assistant-1')
      ]}
      conversation={conversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={onOpenAgent}
      showTokenUsageDetails={false}
    />
  )

  const assistant = screen.container.querySelector('[data-message-id="root-assistant-1"]')
  const nextUser = screen.container.querySelector('[data-message-id="root-user-2"]')
  const started = screen.container.querySelector('[data-semantic="started"]')
  const updated = screen.container.querySelector('[data-semantic="updated"]')
  expect(started?.parentElement?.previousElementSibling).toBe(assistant)
  expect(updated?.parentElement?.nextElementSibling).toBe(nextUser)
  expect(screen.container.querySelector('[data-testid="collaboration-activity"]')).toBeNull()

  expect(
    screen.getByRole('button', { name: 'View sub-agent Reviewer: Started working' })
  ).toBeVisible()
})

it('keeps root collaboration activity out of the child observer capability mode', async () => {
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[activity('event-root-only', 'started', 1, 2_500, null)]}
      conversation={conversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable={false}
      editSelectedModelSupportsImage={false}
      mode="observer"
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelector('[data-testid="collaboration-timeline"]')).toBeNull()
})

it('renders inline activity inside the real interactive ConversationSurface design context', async () => {
  await page.viewport(900, 700)
  const theme = {
    ...getFrontendCssVariables(frontendConfig, classicLightTheme),
    '--main-panel': 'var(--mc-color-surface-main-panel)',
    background: 'var(--mc-color-surface-main-panel)',
    fontFamily: 'var(--mc-font-family)',
    height: 560,
    width: 720
  } as CSSProperties
  const screen = await render(
    <div style={theme}>
      <ConversationSurface
        collaborationTimelineActivities={[
          activity('event-surface', 'started', 1, 2_100, 'root-assistant-1')
        ]}
        composerDraft={{
          attachments: [],
          message: '',
          modelId: 'model-root',
          permissionMode: 'default',
          projectId: 'project-a',
          queuedMessages: [],
          skills: [],
          updatedAt: 5_000
        }}
        conversation={conversation()}
        editSelectedModelAvailable
        editSelectedModelSupportsImage
        mode="interactive"
        onComposerDraftChange={vi.fn()}
        onOpenCollaborationAgent={vi.fn()}
        onSubmitMessage={vi.fn()}
        permissionModeAvailability={{ custom: true, full: true }}
        showTokenUsageDetails={false}
      />
    </div>
  )

  const row = screen.container.querySelector<HTMLElement>('.collaboration-timeline__activity')
  expect(row).not.toBeNull()
  expect(row?.getBoundingClientRect().height).toBeLessThan(34)
  expect(screen.getByText('I delegated the review.')).toBeVisible()
  expect(screen.getByText('Continue.')).toBeVisible()
  await page.screenshot({
    element: screen.container.firstElementChild as HTMLElement,
    path: '__screenshots__/CollaborationTimelineIntegration.browser.test.tsx/root-surface-inline.png'
  })
})
