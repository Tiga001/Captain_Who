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
  'agent.processed': 'Processed in {duration}',
  'chat.copyMessage': 'Copy message',
  'chat.messageActions': 'Message actions',
  'agent.contextCompaction.running': 'Compacting context',
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

vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    git: {
      getTurnDiffSummaries: vi.fn().mockResolvedValue({ summaries: [] })
    }
  }
}))
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
        content: 'I finished the root response.',
        createdAt: 2_000,
        status: 'sent',
        agentRun: {
          runId: 'root-run-1',
          status: 'completed',
          startedAt: 2_000,
          completedAt: 4_000,
          toolDefinitions: [],
          toolCalls: [
            {
              id: 'spawn-reviewer',
              tool: 'spawn_agent',
              args: { task_name: 'Reviewer', message: 'Review the change.' },
              approvalStatus: 'not_required',
              reason: null
            }
          ],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: [
            {
              id: 'trace-message-0',
              type: 'message',
              content: 'I will delegate the review.',
              traceSequence: 0
            },
            {
              id: 'tool-call-spawn-reviewer',
              type: 'tool_call',
              callId: 'spawn-reviewer',
              traceSequence: 1
            },
            {
              id: 'trace-message-2',
              type: 'message',
              content: 'I continued after delegation.',
              traceSequence: 2
            }
          ]
        }
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
    rootTraceBoundarySequence: rootAnchorMessageId === null ? null : sequence,
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
        activity('event-anchored', 'started', 2, 2_100, 'root-assistant-1')
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
  expect(assistant?.contains(started)).toBe(true)
  const before = screen.getByText('I will delegate the review.').element()
  const after = screen.getByText('I continued after delegation.').element()
  expect(
    before.compareDocumentPosition(started!).valueOf() & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
  expect(started!.compareDocumentPosition(after).valueOf() & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
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

it('freezes live activity at its arrival boundary while later root narration keeps streaming', async () => {
  const initial = structuredClone(conversation())
  const initialRun = initial.messages[1]?.agentRun
  if (!initialRun) throw new Error('missing root run fixture')
  initialRun.status = 'running'
  initialRun.completedAt = undefined
  initialRun.timeline = initialRun.timeline
    .slice(0, 2)
    .map((item) => ({ ...item, traceSequence: undefined }))

  const activityAtSpawn = activity('event-live-started', 'started', 2, 2_100, 'root-assistant-1')
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[activityAtSpawn]}
      conversation={initial}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const later = structuredClone(initial)
  const laterRun = later.messages[1]?.agentRun
  if (!laterRun) throw new Error('missing root run fixture')
  laterRun.timeline.push({
    id: 'live-message-after-spawn',
    type: 'message',
    content: 'This narration arrived after the activity.'
  })
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={[activityAtSpawn]}
      conversation={later}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const started = screen.container.querySelector<HTMLElement>('[data-semantic="started"]')
  const laterNarration = screen.getByText('This narration arrived after the activity.').element()
  expect(started).not.toBeNull()
  expect(started!.compareDocumentPosition(laterNarration) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
})

it('keeps an unnumbered presentation item visible before the activity from its arrival cut', async () => {
  const initial = structuredClone(conversation())
  const initialRun = initial.messages[1]?.agentRun
  if (!initialRun) throw new Error('missing root run fixture')
  initialRun.status = 'running'
  initialRun.completedAt = undefined
  initialRun.timeline = [
    ...initialRun.timeline.slice(0, 2),
    {
      id: 'context-compaction-visible-at-arrival',
      type: 'context_compaction',
      operationId: 'compaction-1',
      status: 'running'
    }
  ]

  const activityAtSpawn = activity(
    'event-live-mixed-started',
    'started',
    2,
    2_100,
    'root-assistant-1'
  )
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[activityAtSpawn]}
      conversation={initial}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const started = screen.container.querySelector<HTMLElement>('[data-semantic="started"]')
  const compaction = screen.getByText('Compacting context').element()
  expect(started).not.toBeNull()
  expect(compaction.compareDocumentPosition(started!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )

  const later = structuredClone(initial)
  const laterRun = later.messages[1]?.agentRun
  if (!laterRun) throw new Error('missing root run fixture')
  laterRun.timeline.push({
    id: 'live-message-after-mixed-activity',
    type: 'message',
    content: 'This mixed narration arrived after the activity.'
  })
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={[activityAtSpawn]}
      conversation={later}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const laterNarration = screen
    .getByText('This mixed narration arrived after the activity.')
    .element()
  expect(started!.compareDocumentPosition(laterNarration) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
})

it('keeps an activity after the durable final answer when its boundary follows that trace item', async () => {
  const settled = structuredClone(conversation())
  const run = settled.messages[1]?.agentRun
  if (!run) throw new Error('missing root run fixture')
  run.timeline.push({
    id: 'trace-message-final-answer',
    type: 'message',
    content: 'I finished the root response.',
    traceSequence: 3
  })

  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[
        activity('event-after-final-answer', 'updated', 4, 4_100, 'root-assistant-1')
      ]}
      conversation={settled}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const finalAnswer = screen.getByText('I finished the root response.').element()
  const updated = screen.container.querySelector<HTMLElement>('[data-semantic="updated"]')
  expect(updated).not.toBeNull()
  expect(finalAnswer.compareDocumentPosition(updated!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(screen.getByText('I finished the root response.').elements()).toHaveLength(1)
})

it('keeps activity visible while collapsed and prevents tool groups from merging across it', async () => {
  const grouped = structuredClone(conversation())
  const run = grouped.messages[1]?.agentRun
  if (!run) throw new Error('missing root run fixture')
  run.toolCalls = [
    {
      id: 'read-before-activity',
      tool: 'read_file',
      args: { path: 'before.md' },
      approvalStatus: 'not_required',
      reason: null
    },
    {
      id: 'read-after-activity',
      tool: 'read_file',
      args: { path: 'after.md' },
      approvalStatus: 'not_required',
      reason: null
    }
  ]
  run.toolResults = [
    {
      callId: 'read-before-activity',
      tool: 'read_file',
      ok: true,
      result: { path: 'before.md', content: 'ok' }
    },
    {
      callId: 'read-after-activity',
      tool: 'read_file',
      ok: true,
      result: { path: 'after.md', content: 'ok' }
    }
  ]
  run.timeline = [
    {
      id: 'tool-call-read-before',
      type: 'tool_call',
      callId: 'read-before-activity',
      traceSequence: 0
    },
    {
      id: 'tool-call-read-after',
      type: 'tool_call',
      callId: 'read-after-activity',
      traceSequence: 1
    }
  ]

  const onMessageUiStateChange = vi.fn()
  const activities = [activity('event-between-read-tools', 'started', 1, 2_100, 'root-assistant-1')]
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={activities}
      conversation={grouped}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onMessageUiStateChange={onMessageUiStateChange}
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(1)
  expect(screen.container.querySelectorAll('.agent-activity--read')).toHaveLength(0)

  const disclosure = screen.container.querySelector<HTMLButtonElement>('.agent-run__elapsed-button')
  expect(disclosure).not.toBeNull()
  disclosure?.click()
  expect(onMessageUiStateChange).toHaveBeenCalledWith('root-assistant-1', {
    timelineCollapsed: false
  })
  const expanded = structuredClone(grouped)
  const expandedMessage = expanded.messages[1]
  if (!expandedMessage) throw new Error('missing expanded root message fixture')
  expandedMessage.uiState = { timelineCollapsed: false }
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={activities}
      conversation={expanded}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onMessageUiStateChange={onMessageUiStateChange}
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )
  await expect.poll(() => screen.container.querySelectorAll('.agent-activity--read').length).toBe(2)

  const reads = screen.container.querySelectorAll('.agent-activity--read')
  const started = screen.container.querySelector<HTMLElement>('[data-semantic="started"]')
  expect(started).not.toBeNull()
  expect(reads[0]?.compareDocumentPosition(started!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(started!.compareDocumentPosition(reads[1]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(1)
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
  expect(screen.getByText('I finished the root response.')).toBeVisible()
  expect(screen.getByText('Continue.')).toBeVisible()
  await page.screenshot({
    element: screen.container.firstElementChild as HTMLElement,
    path: '__screenshots__/CollaborationTimelineIntegration.browser.test.tsx/root-surface-inline.png'
  })
})
