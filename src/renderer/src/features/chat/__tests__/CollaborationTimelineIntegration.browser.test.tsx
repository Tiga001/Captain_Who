import type { CSSProperties } from 'react'
import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
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
  'collaboration.activity.agentNameSeparator': ', ',
  'collaboration.activity.copyAgentStatus': '{name}: {status}',
  'collaboration.activity.moreAgents': '{count} more',
  'collaboration.activity.moreAgentsLabel': '{count} more sub-agents: {agents}, {status}',
  'collaboration.activity.openAgentActivity': 'View sub-agent {name}: {status}',
  'collaboration.activity.statusListSeparator': '; ',
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

function copySelection(target: Element): string {
  const clipboardData = new DataTransfer()
  target.dispatchEvent(
    new ClipboardEvent('copy', {
      bubbles: true,
      cancelable: true,
      clipboardData
    })
  )
  return clipboardData.getData('text/plain')
}

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

function expandedConversation(): ChatConversation {
  const expanded = conversation()
  const assistant = expanded.messages[1]
  if (!assistant) throw new Error('missing root Assistant fixture')
  assistant.uiState = { timelineCollapsed: false }
  return expanded
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

it('merges only trusted anchored activity into the message timeline', async () => {
  const onOpenAgent = vi.fn()
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[
        activity('event-unanchored', 'updated', 2, 3_000, null),
        activity('event-anchored', 'started', 2, 2_100, 'root-assistant-1')
      ]}
      conversation={expandedConversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={onOpenAgent}
      showTokenUsageDetails={false}
    />
  )

  const assistant = screen.container.querySelector('[data-message-id="root-assistant-1"]')
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
  expect(updated).toBeNull()
  expect(screen.container.querySelector('[data-testid="collaboration-activity"]')).toBeNull()

  expect(
    screen.getByRole('button', { name: 'View sub-agent Reviewer: Started working' })
  ).toBeVisible()
})

it('does not append or rewrite inline state when post-terminal activity arrives unanchored', async () => {
  const startedBeforeTerminal = activity(
    'started-before-terminal',
    'started',
    1,
    2_100,
    'root-assistant-1'
  )
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[startedBeforeTerminal]}
      conversation={expandedConversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(1)
  expect(screen.container.querySelectorAll('[data-semantic="completed"]')).toHaveLength(0)

  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={[
        startedBeforeTerminal,
        activity('completed-after-terminal', 'completed', 2, 4_500, null)
      ]}
      conversation={expandedConversation()}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(1)
  expect(screen.container.querySelectorAll('[data-semantic="completed"]')).toHaveLength(0)
})

it('keeps root collaboration activity out of the child observer capability mode', async () => {
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[
        activity('event-root-only', 'started', 1, 2_500, 'root-assistant-1')
      ]}
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

it('renders a causally pre-terminal anchored event even when its notification arrives late', async () => {
  const settled = structuredClone(expandedConversation())
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
      collaborationTimelineActivities={[]}
      conversation={settled}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelector('[data-semantic="updated"]')).toBeNull()
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={[
        activity('event-before-terminal-delivered-late', 'updated', 4, 4_100, 'root-assistant-1')
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

it('folds anchored activity with the execution timeline and omits unanchored activity', async () => {
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
  const activities = [
    activity('event-between-read-tools', 'started', 1, 2_100, 'root-assistant-1'),
    activity('event-unanchored-followup', 'updated', 2, 4_500, null)
  ]
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

  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(0)
  expect(screen.container.querySelectorAll('[data-semantic="updated"]')).toHaveLength(0)
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
  expect(screen.container.querySelectorAll('[data-semantic="updated"]')).toHaveLength(0)

  const collapsedAgain = structuredClone(expanded)
  const collapsedAgainMessage = collapsedAgain.messages[1]
  if (!collapsedAgainMessage) throw new Error('missing collapsed root message fixture')
  collapsedAgainMessage.uiState = { timelineCollapsed: true }
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={activities}
      conversation={collapsedAgain}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onMessageUiStateChange={onMessageUiStateChange}
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )
  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(0)
  expect(screen.container.querySelectorAll('[data-semantic="updated"]')).toHaveLength(0)
  expect(screen.container.querySelectorAll('.agent-activity--read')).toHaveLength(0)
})

it('coalesces consecutive same-status Harness activity across hidden trace boundaries', async () => {
  const grouped = structuredClone(expandedConversation())
  const run = grouped.messages[1]?.agentRun
  if (!run) throw new Error('missing root run fixture')
  run.toolCalls = ['agent-a', 'agent-b', 'agent-c'].map((agentId) => ({
    id: `spawn-${agentId}`,
    tool: 'spawn_agent' as const,
    args: { task_name: agentId, message: `Delegate to ${agentId}.` },
    approvalStatus: 'not_required' as const,
    reason: null
  }))
  run.timeline = [
    {
      id: 'trace-before-spawns',
      type: 'message',
      content: 'I will create three sub-agents.',
      traceSequence: 0
    },
    ...['agent-a', 'agent-b', 'agent-c'].map((agentId, index) => ({
      id: `trace-spawn-${agentId}`,
      type: 'tool_call' as const,
      callId: `spawn-${agentId}`,
      traceSequence: index + 1
    })),
    {
      id: 'trace-after-spawns',
      type: 'message',
      content: 'All three are working.',
      traceSequence: 4
    }
  ]
  const onOpenAgent = vi.fn()
  const activities = ['agent-a', 'agent-b', 'agent-c'].map((agentId, index) => ({
    ...activity(`event-${agentId}`, 'started', index + 1, 2_100 + index, 'root-assistant-1'),
    agentId,
    rootTraceBoundarySequence: index + 2,
    taskNameSnapshot: `Worker ${index + 1}`
  }))

  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={activities}
      conversation={grouped}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={onOpenAgent}
      showTokenUsageDetails={false}
    />
  )

  const rows = screen.container.querySelectorAll<HTMLElement>('.collaboration-timeline__activity')
  expect(rows).toHaveLength(1)
  expect(rows[0]?.querySelectorAll('.collaboration-timeline__chip')).toHaveLength(3)
  expect(screen.getByText('Started working').elements()).toHaveLength(1)
  expect(getComputedStyle(rows[0]!.parentElement!).marginBlockStart).toBe('0px')
  await userEvent.click(
    screen.getByRole('button', {
      name: 'View sub-agent Worker 2: Started working'
    })
  )
  expect(onOpenAgent).toHaveBeenCalledWith('agent-b')
})

it('keeps different statuses compact but never merges across visible narration', async () => {
  const grouped = structuredClone(expandedConversation())
  const run = grouped.messages[1]?.agentRun
  if (!run) throw new Error('missing root run fixture')
  run.timeline = [
    {
      id: 'hidden-spawn-a',
      type: 'tool_call',
      callId: 'spawn-reviewer',
      traceSequence: 0
    },
    {
      id: 'visible-narration',
      type: 'message',
      content: 'A visible update separates the events.',
      traceSequence: 2
    },
    {
      id: 'hidden-spawn-b',
      type: 'tool_call',
      callId: 'spawn-reviewer',
      traceSequence: 4
    }
  ]
  const activities = [
    activity('event-start-before-text', 'started', 1, 2_100, 'root-assistant-1'),
    activity('event-update-before-text', 'updated', 2, 2_200, 'root-assistant-1'),
    activity('event-start-after-text', 'started', 5, 2_300, 'root-assistant-1')
  ]

  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={activities}
      conversation={grouped}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const lists = screen.container.querySelectorAll<HTMLElement>('.collaboration-timeline')
  const started = screen.container.querySelectorAll<HTMLElement>('[data-semantic="started"]')
  const updated = screen.container.querySelector<HTMLElement>('[data-semantic="updated"]')
  const narration = screen.getByText('A visible update separates the events.').element()
  expect(lists).toHaveLength(2)
  expect(started).toHaveLength(2)
  expect(updated).not.toBeNull()
  expect(getComputedStyle(lists[0]!).marginBlockStart).toBe('0px')
  expect(getComputedStyle(lists[0]!).rowGap).toBe('6px')
  expect(started[0]!.compareDocumentPosition(updated!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(updated!.compareDocumentPosition(narration) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(narration.compareDocumentPosition(started[1]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
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
        conversation={expandedConversation()}
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
  const messages = screen.container.querySelector<HTMLElement>('.chat-conversation-page__messages')
  const selection = window.getSelection()
  const range = document.createRange()
  range.selectNodeContents(messages!)
  selection?.removeAllRanges()
  selection?.addRange(range)
  const copiedConversation = copySelection(messages!)
  expect(copiedConversation).toContain('I will delegate the review.')
  expect(copiedConversation).toContain('Reviewer: Started working')
  expect(copiedConversation).toContain('I finished the root response.')
  selection?.removeAllRanges()
  await page.screenshot({
    element: screen.container.firstElementChild as HTMLElement,
    path: '__screenshots__/CollaborationTimelineIntegration.browser.test.tsx/root-surface-inline.png'
  })
})
