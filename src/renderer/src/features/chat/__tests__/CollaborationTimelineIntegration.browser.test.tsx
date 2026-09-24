import type { CSSProperties } from 'react'
import type {
  AgentSummary,
  AgentTreeSnapshot,
  CollaborationEventEnvelope
} from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicLightTheme } from '../../../config/themes/classic'
import { ChatMessageList, ConversationSurface } from '../ChatConversationPage'
import type { ChatConversation } from '../chatTypes'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import type { CollaborationDataSource } from '../../agentCollaboration/collaborationClient'
import { CollaborationStore } from '../../agentCollaboration/collaborationStore'
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
          fileChangeProposals: [],
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
  anchorMessageId: string | null
): CollaborationTimelineActivity {
  return {
    activityId,
    agentId: 'agent-reviewer',
    occurredAt,
    ownerAgentId: 'root:root-conversation',
    ownerConversationId: 'root-conversation',
    anchorMessageId,
    traceBoundarySequence: anchorMessageId === null ? null : sequence,
    runId: `run-${sequence}`,
    semantic,
    sequence,
    taskNameSnapshot: 'Reviewer',
    turnId: `turn-${sequence}`,
    taskMessageId: semantic === 'updated' ? null : 'task-agent-reviewer'
  }
}

function freezeTerminalActivities(
  target: ChatConversation,
  activities: readonly CollaborationTimelineActivity[]
): ChatConversation {
  const run = target.messages.find((message) => message.id === 'root-assistant-1')?.agentRun
  if (!run) throw new Error('missing root run fixture')
  run.collaborationTimelineActivities = activities.filter(
    (candidate) =>
      candidate.anchorMessageId === 'root-assistant-1' && candidate.traceBoundarySequence !== null
  )
  return target
}

it('merges only trusted anchored activity into the message timeline', async () => {
  const onOpenAgent = vi.fn()
  const anchored = activity('event-anchored', 'started', 2, 2_100, 'root-assistant-1')
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[
        {
          ...activity('event-grandchild', 'updated', 2, 3_000, null),
          ownerConversationId: 'child-conversation'
        },
        anchored
      ]}
      conversation={freezeTerminalActivities(expandedConversation(), [anchored])}
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

it('places post-terminal activity after the reply and before the next message', async () => {
  const startedBeforeTerminal = activity(
    'started-before-terminal',
    'started',
    1,
    2_100,
    'root-assistant-1'
  )
  const settled = freezeTerminalActivities(expandedConversation(), [startedBeforeTerminal])
  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[startedBeforeTerminal]}
      conversation={settled}
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
        {
          ...activity('completed-after-terminal', 'completed', 2, 4_500, 'root-assistant-1'),
          traceBoundarySequence: null
        }
      ]}
      conversation={settled}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  expect(screen.container.querySelectorAll('[data-semantic="started"]')).toHaveLength(1)
  const completed = screen.container.querySelector('[data-semantic="completed"]')!
  const assistant = screen.container.querySelector('[data-message-id="root-assistant-1"]')!
  const nextUser = screen.container.querySelector('[data-message-id="root-user-2"]')!
  expect(assistant.contains(completed)).toBe(false)
  expect(
    assistant.compareDocumentPosition(completed) & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  expect(
    completed.compareDocumentPosition(nextUser) & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
})

it('consumes a filtered Result wake without adding a footer card and still displays a later real followup completion', async () => {
  const started = activity('task-started', 'started', 1, 2_100, 'root-assistant-1')
  const child: AgentSummary = {
    agentId: 'agent-reviewer',
    conversationId: 'reviewer-conversation',
    displayStatus: 'running',
    latestActivityAt: 2_100,
    lifecycle: 'active',
    model: { displayName: 'Review model', modelConfigId: 'review-model' },
    parentAgentId: 'root:root-conversation',
    projectId: 'project-a',
    rootAgentId: 'root:root-conversation',
    rootConversationId: 'root-conversation',
    taskName: 'Reviewer',
    taskPath: '/root/reviewer'
  }
  let authoritative: AgentTreeSnapshot = {
    schemaVersion: 1,
    workspaceId: 'project-a',
    projectId: 'project-a',
    rootAgentId: 'root:root-conversation',
    rootConversationId: 'root-conversation',
    agents: [child],
    lastSequence: 1
  }
  const hostEvent = (
    sequence: number,
    projected: CollaborationTimelineActivity | null,
    runId: string
  ): CollaborationEventEnvelope => ({
    schemaVersion: 3,
    eventId: projected?.activityId ?? 'result-wake-completed',
    sequence,
    workspaceId: 'project-a',
    projectId: 'project-a',
    rootAgentId: authoritative.rootAgentId,
    rootConversationId: authoritative.rootConversationId,
    agentId: child.agentId,
    conversationId: child.conversationId,
    turnId: `turn-${runId}`,
    runId,
    messageId: `source-${runId}`,
    kind: projected?.semantic === 'started' ? 'wake_created' : 'wake_updated',
    resourceRevision: sequence,
    activities: projected
      ? [
          {
            schemaVersion: 4,
            activityId: projected.activityId,
            agentId: projected.agentId,
            ownerAgentId: projected.ownerAgentId,
            ownerConversationId: projected.ownerConversationId,
            anchorMessageId: projected.anchorMessageId,
            traceBoundarySequence: projected.traceBoundarySequence,
            semantic: projected.semantic,
            taskNameSnapshot: projected.taskNameSnapshot,
            taskMessageId: projected.taskMessageId
          }
        ]
      : [],
    occurredAt: projected?.occurredAt ?? 4_200
  })
  const durable = [hostEvent(1, started, 'task-run')]
  let notify: ((event: CollaborationEventEnvelope) => void) | undefined
  const source: CollaborationDataSource = {
    getTree: vi.fn(async () => authoritative),
    listEvents: vi.fn(async ({ afterSequence }) => ({
      schemaVersion: 1 as const,
      rootAgentId: authoritative.rootAgentId,
      rootConversationId: authoritative.rootConversationId,
      events: durable.filter((event) => event.sequence > afterSequence),
      lastSequence: authoritative.lastSequence,
      hasMore: false
    })),
    subscribe: (handler) => {
      notify = handler
      return () => undefined
    },
    subscribeResync: () => () => undefined
  }
  const store = new CollaborationStore('root-conversation', source)
  store.start()
  try {
    await expect.poll(() => store.getSnapshot().loading).toBe(false)
    const settled = freezeTerminalActivities(expandedConversation(), [started])
    const view = () => (
      <ChatMessageList
        collaborationTimelineActivities={store.getSnapshot().activities}
        conversation={settled}
        collaborationTreeAgentIds={['root:root-conversation', 'agent-reviewer']}
        editableLastUserMessageId={null}
        editSelectedModelAvailable
        editSelectedModelSupportsImage
        onOpenCollaborationAgent={vi.fn()}
        showTokenUsageDetails={false}
      />
    )
    const screen = await render(view())
    const footer = screen.container.querySelector('.chat-message__actions')!
    expect(footer).not.toBeNull()
    expect(screen.getByText('I finished the root response.').elements()).toHaveLength(1)

    // Host retains the durable event and state change, but suppresses the notification-only
    // Result wake's presentation activity instead of deleting a sequence from the event stream.
    const notification = hostEvent(2, null, 'result-notification-run')
    durable.push(notification)
    authoritative = {
      ...authoritative,
      lastSequence: 2,
      agents: [{ ...child, displayStatus: 'latest_completed', latestActivityAt: 4_200 }]
    }
    notify?.(notification)
    await expect.poll(() => store.getSnapshot().tree?.lastSequence).toBe(2)
    expect(store.getSnapshot().tree?.agents[0]?.displayStatus).toBe('latest_completed')
    expect(store.getSnapshot().agentInvalidationSequences[child.agentId]).toBe(2)
    expect(store.getSnapshot().activities.map((event) => event.activityId)).toEqual([
      'task-started'
    ])
    await screen.rerender(view())
    expect(screen.container.querySelector('[data-semantic="completed"]')).toBeNull()

    const followupStarted = {
      ...activity('followup-started', 'started', 3, 4_500, 'root-assistant-1'),
      traceBoundarySequence: null
    }
    const followupCompleted = {
      ...activity('followup-completed', 'completed', 4, 4_800, 'root-assistant-1'),
      traceBoundarySequence: null
    }
    durable.push(
      hostEvent(3, followupStarted, 'followup-run'),
      hostEvent(4, followupCompleted, 'followup-run')
    )
    authoritative = {
      ...authoritative,
      lastSequence: 4,
      agents: [{ ...child, displayStatus: 'latest_completed', latestActivityAt: 4_800 }]
    }
    notify?.(durable[3])
    await expect.poll(() => store.getSnapshot().tree?.lastSequence).toBe(4)
    expect(source.listEvents).toHaveBeenLastCalledWith({
      afterSequence: 2,
      limit: 256,
      rootConversationId: 'root-conversation'
    })
    expect(store.getSnapshot().error).toBe(false)
    await screen.rerender(view())
    const completed = screen.container.querySelector('[data-semantic="completed"]')!
    expect(screen.container.querySelectorAll('[data-semantic="completed"]')).toHaveLength(1)
    expect(
      footer.compareDocumentPosition(completed) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    const nextUser = screen.container.querySelector('[data-message-id="root-user-2"]')!
    expect(
      completed.compareDocumentPosition(nextUser) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(store.getSnapshot().activities.map((event) => event.activityId)).toEqual([
      'task-started',
      'followup-started',
      'followup-completed'
    ])
  } finally {
    store.destroy()
  }
})

it('places an observer’s direct-child status inline and completion between turns', async () => {
  const onOpenAgent = vi.fn()
  const started = {
    ...activity('event-child-started', 'started', 2, 2_500, 'root-assistant-1'),
    ownerAgentId: 'parent-agent',
    ownerConversationId: 'child-conversation'
  }
  const completed = {
    ...started,
    activityId: 'event-child-completed',
    sequence: 3,
    semantic: 'completed' as const,
    traceBoundarySequence: null
  }
  const observer = freezeTerminalActivities(expandedConversation(), [started])
  observer.id = 'child-conversation'
  const siblingEvent = activity('event-sibling', 'failed', 4, 9_000, 'root-assistant-1')
  // Both live and terminal projections reject another parent’s events, even with a matching ID.
  observer.messages[1]!.agentRun!.collaborationTimelineActivities!.push(siblingEvent)
  const screen = await render(
    <div className="chat-conversation-page__messages">
      <ChatMessageList
        collaborationTimelineActivities={[started, completed, siblingEvent]}
        conversation={observer}
        editableLastUserMessageId={null}
        editSelectedModelAvailable={false}
        editSelectedModelSupportsImage={false}
        mode="observer"
        onOpenCollaborationAgent={onOpenAgent}
        showTokenUsageDetails={false}
      />
    </div>
  )

  const statuses = screen.container.querySelectorAll('[data-semantic]')
  expect(Array.from(statuses, (status) => status.getAttribute('data-semantic'))).toEqual([
    'started',
    'completed'
  ])
  const assistant = screen.container.querySelector('[data-message-id="root-assistant-1"]')!
  const before = screen.getByText('I will delegate the review.').element()
  const after = screen.getByText('I continued after delegation.').element()
  expect(assistant.contains(statuses[0]!)).toBe(true)
  expect(
    before.compareDocumentPosition(statuses[0]!) & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  expect(
    statuses[0]!.compareDocumentPosition(after) & Node.DOCUMENT_POSITION_FOLLOWING
  ).toBeTruthy()
  expect(assistant.contains(statuses[1]!)).toBe(false)
  await userEvent.click(screen.getByRole('button', { name: 'View sub-agent Reviewer: Completed' }))
  expect(onOpenAgent).toHaveBeenCalledWith('agent-reviewer')
})

it.each(['interactive', 'observer'] as const)(
  'hides inherited fork activities outside the authoritative tree membership in %s mode',
  async (mode) => {
    const stale = {
      ...activity('inherited-event', 'failed', 1, 1_000, 'root-assistant-1'),
      agentId: 'old-tree-child',
      ownerAgentId: 'old-root-agent',
      ownerConversationId: 'fork-conversation',
      taskNameSnapshot: 'Old child'
    }
    const current = {
      ...activity('current-event', 'started', 2, 2_000, 'root-assistant-1'),
      agentId: 'current-child',
      ownerAgentId: 'current-root-agent',
      ownerConversationId: 'fork-conversation',
      taskNameSnapshot: 'Current child'
    }
    const fork = freezeTerminalActivities(expandedConversation(), [stale, current])
    fork.id = 'fork-conversation'
    const activities = [
      stale,
      current,
      { ...stale, activityId: 'inherited-gap', traceBoundarySequence: null }
    ]
    const props = {
      conversation: fork,
      collaborationTimelineActivities: activities,
      editableLastUserMessageId: null,
      editSelectedModelAvailable: false,
      editSelectedModelSupportsImage: false,
      mode,
      onOpenCollaborationAgent: vi.fn(),
      showTokenUsageDetails: false
    }
    const screen = await render(<ChatMessageList {...props} collaborationTreeAgentIds={[]} />)
    expect(screen.container.querySelectorAll('[data-semantic]')).toHaveLength(0)

    await screen.rerender(
      <ChatMessageList
        {...props}
        collaborationTreeAgentIds={['current-root-agent', 'current-child']}
      />
    )
    expect(screen.container.querySelectorAll('[data-semantic]')).toHaveLength(1)
    expect(screen.container.querySelector('[data-agent-id="current-child"]')).not.toBeNull()
    expect(screen.container.querySelector('[data-agent-id="old-tree-child"]')).toBeNull()
  }
)

it('shows cross-level task ownership consistently in frozen root and observer conversations without subscribing ordinary messages to completion', async () => {
  const members = ['root:root-conversation', 'parent', 'grandchild']
  const rootTask = {
    ...activity('root-task-started', 'started', 1, 2_000, 'root-assistant-1'),
    agentId: 'grandchild',
    taskNameSnapshot: 'Grandchild',
    taskMessageId: 'root-task'
  }
  const rootFollowup = {
    ...rootTask,
    activityId: 'root-followup-started',
    taskMessageId: 'root-followup',
    sequence: 2
  }
  const parentTask = {
    ...activity('parent-task-completed', 'completed', 3, 3_000, 'parent-assistant-1'),
    agentId: 'grandchild',
    taskNameSnapshot: 'Grandchild',
    taskMessageId: 'parent-task',
    ownerAgentId: 'parent',
    ownerConversationId: 'parent-conversation'
  }
  const message = {
    ...activity('ordinary-message', 'updated', 4, 3_100, 'root-assistant-1'),
    agentId: 'parent',
    taskNameSnapshot: 'Parent',
    taskMessageId: null
  }
  // The sender's later, unrelated work belongs to its own dispatcher, not the message recipient.
  const unrelatedCompletion = {
    ...activity('unrelated-completed', 'completed', 5, 4_000, 'parent-assistant-1'),
    ownerAgentId: 'parent',
    ownerConversationId: 'parent-conversation',
    agentId: 'grandchild',
    taskNameSnapshot: 'Unrelated task',
    taskMessageId: 'unrelated-task'
  }
  const activities = [rootTask, rootFollowup, parentTask, message, unrelatedCompletion]
  const root = freezeTerminalActivities(expandedConversation(), activities)
  const props = {
    collaborationTreeAgentIds: members,
    collaborationTimelineActivities: activities,
    editableLastUserMessageId: null,
    editSelectedModelAvailable: false,
    editSelectedModelSupportsImage: false,
    onOpenCollaborationAgent: vi.fn(),
    showTokenUsageDetails: false
  }
  const screen = await render(<ChatMessageList {...props} conversation={root} mode="interactive" />)
  expect(
    screen.container.querySelectorAll('[data-semantic="started"] [data-agent-id="grandchild"]')
  ).toHaveLength(2)
  expect(
    screen.container.querySelectorAll('[data-semantic="updated"] [data-agent-id="parent"]')
  ).toHaveLength(1)
  expect(screen.container.querySelectorAll('[data-semantic="completed"]')).toHaveLength(0)

  const parent = freezeTerminalActivities(expandedConversation(), activities)
  parent.id = 'parent-conversation'
  parent.messages[1]!.id = 'parent-assistant-1'
  parent.messages[1]!.agentRun!.collaborationTimelineActivities = [parentTask, unrelatedCompletion]
  await screen.rerender(<ChatMessageList {...props} conversation={parent} mode="observer" />)
  expect(
    screen.container.querySelectorAll('[data-semantic="completed"] [data-agent-id="grandchild"]')
  ).toHaveLength(2)
  expect(
    screen.container.querySelectorAll('[data-semantic="started"], [data-semantic="updated"]')
  ).toHaveLength(0)

  await screen.rerender(
    <ChatMessageList
      {...props}
      conversation={root}
      mode="interactive"
      collaborationTimelineActivities={[]}
    />
  )
  expect(
    screen.container.querySelectorAll('[data-semantic="started"] [data-agent-id="grandchild"]')
  ).toHaveLength(2)
  expect(screen.container.querySelectorAll('[data-semantic="completed"]')).toHaveLength(0)
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

it('keeps the frozen pre-final activity in place and ignores a late live terminal event', async () => {
  const updatedBeforeFinal = activity(
    'event-before-terminal-delivered-late',
    'updated',
    3,
    4_100,
    'root-assistant-1'
  )
  const active = structuredClone(expandedConversation())
  const activeMessage = active.messages[1]
  const activeRun = activeMessage?.agentRun
  if (!activeMessage || !activeRun) throw new Error('missing root run fixture')
  activeMessage.content = ''
  activeMessage.status = 'pending'
  activeRun.status = 'running'
  activeRun.completedAt = undefined

  const screen = await render(
    <ChatMessageList
      collaborationTimelineActivities={[updatedBeforeFinal]}
      conversation={active}
      editableLastUserMessageId={null}
      editSelectedModelAvailable
      editSelectedModelSupportsImage
      onOpenCollaborationAgent={vi.fn()}
      showTokenUsageDetails={false}
    />
  )

  const initialUpdated = screen.container.querySelector<HTMLElement>('[data-semantic="updated"]')
  expect(initialUpdated).not.toBeNull()
  const settled = structuredClone(active)
  const settledMessage = settled.messages[1]
  const settledRun = settledMessage?.agentRun
  if (!settledMessage || !settledRun) throw new Error('missing settled root run fixture')
  settledMessage.content = 'I finished the root response.'
  settledMessage.status = 'sent'
  settledRun.status = 'completed'
  settledRun.completedAt = 4_300
  settledRun.collaborationTimelineActivities = [updatedBeforeFinal]
  settledRun.timeline.push({
    id: 'trace-message-final-answer',
    type: 'message',
    content: 'I finished the root response.'
  })
  await screen.rerender(
    <ChatMessageList
      collaborationTimelineActivities={[
        updatedBeforeFinal,
        activity('event-after-final', 'completed', 4, 4_200, 'root-assistant-1')
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
  expect(updated!.compareDocumentPosition(finalAnswer) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING
  )
  expect(screen.container.querySelector('[data-semantic="completed"]')).toBeNull()
  expect(screen.getByText('I finished the root response.').elements()).toHaveLength(1)
})

it.each([false, true])(
  'preserves status on both sides of final streaming after reload (timeline final: %s)',
  async (hasTimelineFinal) => {
    const beforeFinal = activity('event-before-final', 'updated', 4, 10_000, 'root-assistant-1')
    const duringFinal = {
      ...activity('event-during-final', 'updated', 6, 1_000, 'root-assistant-1'),
      traceBoundarySequence: beforeFinal.traceBoundarySequence
    }
    const settled = freezeTerminalActivities(expandedConversation(), [beforeFinal, duringFinal])
    const run = settled.messages[1]!.agentRun!
    run.collaborationFinalResponseBoundary = 5
    if (hasTimelineFinal) {
      run.timeline.push({
        id: 'trace-final-stream',
        type: 'message',
        content: 'I finished the root response.'
      })
    }
    const screen = await render(
      <ChatMessageList
        conversation={settled}
        collaborationTimelineActivities={[]}
        editableLastUserMessageId={null}
        editSelectedModelAvailable
        editSelectedModelSupportsImage
        onOpenCollaborationAgent={vi.fn()}
        showTokenUsageDetails={false}
      />
    )
    const statuses = screen.container.querySelectorAll('[data-semantic="updated"]')
    const finalAnswer = screen.getByText('I finished the root response.').element()
    expect(statuses).toHaveLength(2)
    expect(
      statuses[0]!.compareDocumentPosition(finalAnswer) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(
      finalAnswer.compareDocumentPosition(statuses[1]!) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    expect(screen.getByText('I finished the root response.').elements()).toHaveLength(1)
  }
)

it('folds inline activity with the execution timeline and excludes another parent’s events', async () => {
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
    {
      ...activity('event-other-parent', 'updated', 2, 4_500, null),
      ownerConversationId: 'other-parent'
    }
  ]
  run.collaborationTimelineActivities = [activities[0]!]
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
    traceBoundarySequence: index + 2,
    taskNameSnapshot: `Worker ${index + 1}`
  }))
  run.collaborationTimelineActivities = activities

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
  run.collaborationTimelineActivities = activities

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
  const surfaceActivity = activity('event-surface', 'started', 1, 2_100, 'root-assistant-1')
  const screen = await render(
    <div style={theme}>
      <ConversationSurface
        collaborationTimelineActivities={[surfaceActivity]}
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
        conversation={freezeTerminalActivities(expandedConversation(), [surfaceActivity])}
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
