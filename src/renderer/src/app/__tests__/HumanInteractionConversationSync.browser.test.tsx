import { useCallback, useLayoutEffect, useRef, useState, type SetStateAction } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  AgentEvent,
  HumanInteractionRequestSnapshot,
  PendingAgentActionSnapshot
} from '@mycopilot/protocol'
import type { ChatConversation, ChatMessage } from '../../features/chat/chatTypes'
import type { AgentRunLifecycleRefs } from '../agentRunLifecycleSupport'
import {
  deferred,
  question
} from '../../features/humanInteraction/__tests__/humanInteractionFixtures'

const client = vi.hoisted(() => ({
  load: vi.fn(),
  start: vi.fn(),
  submit: vi.fn(),
  ignore: vi.fn(),
  save: vi.fn(),
  toast: vi.fn(),
  pendingActions: vi.fn(),
  requestListeners: new Set<(request: HumanInteractionRequestSnapshot) => void>(),
  resyncListeners: new Set<() => void>(),
  agentListeners: new Set<(event: AgentEvent) => void>()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    app: { onFlushBeforeQuit: () => () => {} },
    humanInteraction: {
      submit: client.submit,
      ignore: client.ignore,
      onRequestChanged: (listener: (request: HumanInteractionRequestSnapshot) => void) => {
        client.requestListeners.add(listener)
        return () => client.requestListeners.delete(listener)
      },
      onResync: (listener: () => void) => {
        client.resyncListeners.add(listener)
        return () => client.resyncListeners.delete(listener)
      }
    }
  }
}))
vi.mock('../../features/storage/storageClient', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../features/storage/storageClient')>()),
  loadConversation: client.load,
  saveConversationMeta: client.save,
  loadInputAttachments: async () => []
}))
vi.mock('../../features/agent/agentClient', () => ({
  cancelAgentRun: vi.fn(),
  startConversationTurn: client.start,
  getAgentCommandSession: async () => null,
  listAgentCommandSessions: async () => ({ sessions: [] }),
  listPendingAgentActions: client.pendingActions,
  onAgentEvent: (listener: (event: AgentEvent) => void) => {
    client.agentListeners.add(listener)
    return () => client.agentListeners.delete(listener)
  }
}))
vi.mock('../useRequestAssistantResponse', () => ({
  useRequestAssistantResponse: () => client.start
}))
import { useHumanInteractionConversationSync } from '../useHumanInteractionConversationSync'
import { useAgentRunLifecycle } from '../useAgentRunLifecycle'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducer'

function conversation(id = 'chat', messages: ChatMessage[] = []): ChatConversation {
  return {
    id,
    title: 'Chat',
    modelId: 'model',
    projectId: null,
    messages,
    messagesLoaded: true,
    createdAt: 1,
    updatedAt: 1
  }
}
function assistant(
  status:
    | 'running'
    | 'completed'
    | 'cancelled'
    | 'waiting_for_approval'
    | 'waiting_for_user_input' = 'running',
  content = ''
): ChatMessage {
  return {
    id: 'assistant',
    role: 'assistant',
    content,
    createdAt: 1,
    status: status === 'completed' || status === 'cancelled' ? 'sent' : 'pending',
    agentRun: ensureAgentRun(undefined, 'host-run', status)
  }
}
interface HarnessView {
  conversations: ChatConversation[]
  update(value: SetStateAction<ChatConversation[]>): void
  refs: AgentRunLifecycleRefs
}
function Harness({
  initial,
  activeConversationId = 'chat',
  lifecycle = false,
  onRender
}: {
  initial: ChatConversation[]
  activeConversationId?: string
  lifecycle?: boolean
  onRender(view: HarnessView): void
}) {
  const [conversations, setConversations] = useState(initial)
  const conversationsRef = useRef(initial)
  const update = useCallback((value: SetStateAction<ChatConversation[]>) => {
    const next = typeof value === 'function' ? value(conversationsRef.current) : value
    conversationsRef.current = next
    setConversations(next)
  }, [])
  const [refs] = useState<AgentRunLifecycleRefs>(() => ({
    activeRunBindings: { current: new Map() },
    autoSubmitQueuedMessage: { current: vi.fn() },
    bufferedAgentEvents: { current: new Map() },
    cancelledPendingMessageIds: { current: new Set() },
    cancelledRunIds: { current: new Set() },
    locallyUnconfirmedStoppedRunIds: { current: new Set() },
    pendingActionsHydrated: { current: new Set() },
    pendingGuidancePayloads: { current: new Map() },
    pendingMessageDeltas: { current: new Map() },
    retiredAgentRunIds: { current: new Set() },
    stopReconciliationTimers: { current: new Map() },
    stopRequestedPendingMessageIds: { current: new Set() },
    stopRequestedRunIds: { current: new Set() }
  }))
  useHumanInteractionConversationSync({
    activeConversationId,
    conversationsRef,
    setConversations: update
  })
  useLayoutEffect(() => {
    onRender({ conversations, update, refs })
  }, [conversations, onRender, refs, update])
  return (
    <>
      {lifecycle && (
        <Lifecycle
          conversations={conversations}
          conversationsRef={conversationsRef}
          setConversations={update}
          refs={refs}
        />
      )}
      <output>
        {conversations
          .flatMap((chat) =>
            chat.messages.map(
              (message) => `${message.id}:${message.content}:${message.agentRun?.status ?? ''}`
            )
          )
          .join('|')}
      </output>
    </>
  )
}
function Lifecycle({
  conversations,
  conversationsRef,
  setConversations,
  refs
}: {
  conversations: ChatConversation[]
  conversationsRef: { current: ChatConversation[] }
  setConversations(value: SetStateAction<ChatConversation[]>): void
  refs: AgentRunLifecycleRefs
}) {
  const activeConversationIdRef = useRef<string | null>('chat'),
    draftsRef = useRef({})
  const lifecycle = useAgentRunLifecycle({
    contextWindowIndicatorEnabled: false,
    conversationState: {
      activeConversationId: 'chat',
      activeConversationIdRef,
      conversations,
      conversationsRef,
      setActiveConversationId: vi.fn(),
      setConversations
    },
    draftState: { draftsRef, mutateDraft: vi.fn() },
    refs,
    enqueueChatMessageCheckpoint: client.save,
    enqueueChatMessageStateSave: client.save,
    enqueueConversationMetaSave: vi.fn(),
    flushChatMessageStateSave: async () => {},
    flushConversationMessageStateSaves: async () => {},
    recordContextWindowSnapshot: vi.fn(),
    reconcileFailedSkillActivation: vi.fn(),
    requestSkillCatalogRefresh: vi.fn(),
    sealAndFlushChatMessageStateSaves: async () => {},
    showToast: client.toast,
    t: (key) => key,
    uiPreferences: {
      customPermissions: {
        read: 'workspace_only',
        write: 'workspace_only',
        command: 'require_approval',
        commandSafety: 'guarded',
        patch: 'require_approval',
        builtinExecution: 'require_approval'
      }
    }
  })
  return (
    <button
      onClick={() => {
        const runId = 'host-run'
        const binding = refs.activeRunBindings.current.get(runId)
        if (!binding) return
        refs.stopRequestedRunIds.current.add(runId)
        lifecycle.scheduleStoppedRunReconciliation(runId, binding)
      }}
    >
      Reconcile stopped run
    </button>
  )
}
function notify(id = 'chat') {
  for (const listener of client.requestListeners) listener(question('request', 1, id))
}
beforeEach(() => {
  client.load.mockReset().mockResolvedValue(null)
  client.start.mockReset()
  client.submit.mockReset()
  client.ignore.mockReset()
  client.save.mockReset()
  client.toast.mockReset()
  client.pendingActions.mockReset().mockResolvedValue([])
  client.requestListeners.clear()
  client.resyncListeners.clear()
  client.agentListeners.clear()
})

it.each(['completed', 'cancelled'] as const)(
  'attaches a late organization delivery after a %s run binding is retired without starting new work',
  async (status) => {
    const source = {
      nodeId: 'boss',
      nodeName: 'Boss',
      conversationId: 'boss-chat',
      conversationTitle: 'Boss',
      content: 'Accepted mail'
    }
    let view!: HarnessView
    const screen = await render(
      <Harness
        initial={[
          conversation('chat', [
            assistant(status, 'Final result'),
            {
              id: 'mail-projection',
              role: 'user',
              content: 'Organization wrapper',
              createdAt: 2,
              workflowSource: {
                inputId: 'input',
                instanceId: 'workflow',
                workflowName: 'Review',
                sources: [source]
              }
            }
          ])
        ]}
        lifecycle
        onRender={(next) => {
          view = next
        }}
      />
    )
    await expect.poll(() => client.agentListeners.size).toBe(1)
    view.refs.retiredAgentRunIds.current.add('host-run')
    view.refs.cancelledRunIds.current.add('host-run')
    view.refs.cancelledPendingMessageIds.current.add('assistant')
    view.refs.activeRunBindings.current.delete('host-run')
    const event: AgentEvent = {
      type: 'workflow_delivery_applied',
      conversationId: 'chat',
      assistantMessageId: 'assistant',
      runId: 'host-run',
      inputId: 'input',
      deliveryId: 'mail-projection',
      instanceId: 'workflow',
      workflowName: 'Review',
      content: 'Organization wrapper',
      createdAt: 2,
      sequence: 4,
      sources: [source]
    }
    for (const listener of client.agentListeners) listener(event)
    await expect.poll(() => view.conversations[0].messages).toHaveLength(1)
    expect(view.conversations[0].messages[0].agentRun!.timeline).toEqual([
      expect.objectContaining({ type: 'workflow_delivery', inputId: 'input', traceSequence: 4 })
    ])
    expect(view.conversations[0].messages[0].agentRun!.status).toBe(status)
    expect(view.conversations[0].messages[0].content).toBe('Final result')
    expect(view.refs.activeRunBindings.current.has('host-run')).toBe(false)
    expect(view.refs.bufferedAgentEvents.current.has('host-run')).toBe(false)
    expect(client.start).not.toHaveBeenCalled()
    expect(view.refs.autoSubmitQueuedMessage.current).not.toHaveBeenCalledWith('chat')
    await screen.unmount()
  }
)

it('loads Host-created User and Run identities on notification and replays buffered Run events without starting a model', async () => {
  const initial = conversation()
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[initial]}
      lifecycle
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.agentListeners.size).toBe(1)
  const event: AgentEvent = {
    type: 'message',
    runId: 'host-run',
    content: 'Buffered model response'
  }
  for (const listener of client.agentListeners) listener(event)
  expect(view.refs.bufferedAgentEvents.current.get('host-run')).toEqual([event])
  client.load.mockResolvedValueOnce(
    conversation('chat', [
      { id: 'answer', role: 'user', content: 'Frozen Q + A', createdAt: 2 },
      assistant()
    ])
  )
  notify()
  await expect
    .poll(
      () => view.conversations[0].messages.find((message) => message.id === 'assistant')?.content
    )
    .toBe('Buffered model response')
  expect(view.refs.activeRunBindings.current.get('host-run')).toEqual({
    conversationId: 'chat',
    pendingMessageId: 'assistant'
  })
  expect(view.refs.bufferedAgentEvents.current.has('host-run')).toBe(false)
  expect(view.conversations[0].messages.map((message) => message.id)).toEqual([
    'answer',
    'assistant'
  ])
  expect(client.start).not.toHaveBeenCalled()
  expect(client.submit).not.toHaveBeenCalled()
  expect(client.ignore).not.toHaveBeenCalled()
  await screen.unmount()
})

it('serializes bursts into one trailing refresh and preserves streaming received during a snapshot read', async () => {
  const initial = conversation('chat', [assistant('running', 'Original')]),
    first = deferred<ChatConversation | null>(),
    second = deferred<ChatConversation | null>()
  client.load.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[initial]}
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  notify()
  notify()
  notify()
  view.update((current) =>
    current.map((chat) => ({
      ...chat,
      messages: chat.messages.map((message) => ({ ...message, content: 'New live content' }))
    }))
  )
  first.resolve(initial)
  await expect.poll(() => client.load).toHaveBeenCalledTimes(2)
  await expect.poll(() => view.conversations[0].messages[0].content).toBe('New live content')
  second.resolve(conversation('chat', [assistant('completed', 'Final authoritative response')]))
  await expect.poll(() => view.conversations[0].messages[0].agentRun?.status).toBe('completed')
  expect(client.load).toHaveBeenCalledTimes(2)
  await screen.unmount()
})

it.each(['completed', 'cancelled'] as const)(
  'never regresses an already %s Run to a late stored running projection',
  async (status) => {
    const initial = conversation('chat', [assistant(status, 'Terminal before read')])
    client.load.mockResolvedValue(conversation('chat', [assistant('running', 'Stale snapshot')]))
    let view!: HarnessView
    const screen = await render(
      <Harness
        initial={[initial]}
        onRender={(next) => {
          view = next
        }}
      />
    )
    await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
    await expect.poll(() => view.conversations[0]).not.toBe(initial)
    await expect.poll(() => view.conversations[0].messages[0].agentRun?.status).toBe(status)
    expect(view.conversations[0].messages[0].content).toBe('Terminal before read')
    await screen.unmount()
  }
)

it('merges authoritative terminal progress with a favorite toggled during the read', async () => {
  const initial = conversation('chat', [assistant('running', 'Before')]),
    pending = deferred<ChatConversation | null>()
  client.load.mockReturnValueOnce(pending.promise)
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[initial]}
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  view.update((current) =>
    current.map((chat) => ({
      ...chat,
      messages: chat.messages.map((message) => ({ ...message, uiState: { favorited: true } }))
    }))
  )
  pending.resolve(conversation('chat', [assistant('completed', 'Final')]))
  await expect.poll(() => view.conversations[0].messages[0].agentRun?.status).toBe('completed')
  expect(view.conversations[0].messages[0].uiState?.favorited).toBe(true)
  await screen.unmount()
})

it('reconnects the active and unfinished chats without reloading a large idle history or sidebar-only chats', async () => {
  const loaded = conversation(),
    other = conversation('other', [assistant()]),
    idle = Array.from({ length: 80 }, (_, index) => conversation(`idle-${index}`)),
    unloaded = { ...conversation('sidebar'), messagesLoaded: false }
  const screen = await render(
    <Harness initial={[loaded, ...idle, other, unloaded]} onRender={() => {}} />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  client.load.mockClear()
  for (const listener of client.resyncListeners) listener()
  await expect.poll(() => client.load).toHaveBeenCalledTimes(2)
  expect(client.load.mock.calls.map(([id]) => id)).toEqual(['chat', 'other'])
  client.load.mockClear()
  window.dispatchEvent(new Event('focus'))
  await expect.poll(() => client.load).toHaveBeenCalledExactlyOnceWith('chat')
  await screen.unmount()
  expect(client.requestListeners.size).toBe(0)
  expect(client.resyncListeners.size).toBe(0)
})

it('keeps in-flight reads across navigation and refreshes idle history when it becomes active', async () => {
  const first = deferred<ChatConversation | null>()
  const initial = [conversation(), conversation('other')]
  client.load.mockReturnValueOnce(first.promise)
  const onRender = () => {}
  const screen = await render(<Harness initial={initial} onRender={onRender} />)
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  await screen.rerender(
    <Harness initial={initial} activeConversationId="other" onRender={onRender} />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(2)
  expect(client.load.mock.calls[1]).toEqual(['other'])
  await screen.rerender(<Harness initial={initial} onRender={onRender} />)
  expect(client.load).toHaveBeenCalledTimes(2)
  first.resolve(initial[0])
  await expect.poll(() => client.load).toHaveBeenCalledTimes(3)
  expect(client.load.mock.calls[2]).toEqual(['chat'])
  expect(client.requestListeners.size).toBe(1)
  await screen.unmount()
})

it('coalesces updates during a failed read into its backoff instead of retrying immediately', async () => {
  const first = deferred<ChatConversation | null>()
  client.load.mockReturnValueOnce(first.promise)
  const screen = await render(<Harness initial={[conversation()]} onRender={() => {}} />)
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  vi.useFakeTimers()
  try {
    notify()
    first.reject(new Error('Read queue overloaded'))
    await vi.advanceTimersByTimeAsync(0)
    notify()
    notify()
    await vi.advanceTimersByTimeAsync(249)
    expect(client.load).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(client.load).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(client.load).toHaveBeenCalledTimes(2)
  } finally {
    vi.useRealTimers()
    await screen.unmount()
  }
})

it('retries a reconnect read failure, but discards late responses after unmount', async () => {
  const pending = deferred<ChatConversation | null>()
  client.load.mockRejectedValueOnce(new Error('Reconnecting')).mockReturnValueOnce(pending.promise)
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[conversation()]}
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(2)
  await screen.unmount()
  pending.resolve(conversation('chat', [assistant()]))
  await Promise.resolve()
  expect(view.conversations[0].messages).toEqual([])
  expect(client.start).not.toHaveBeenCalled()
})

it('keeps an authoritative reconnect terminal when outstanding stop reconciliation reads fail', async () => {
  const running = conversation('chat', [assistant('running', 'Work before Core restarted')])
  const terminal = {
    ...assistant('running', 'Durable partial response'),
    status: 'error' as const,
    agentRun: {
      ...ensureAgentRun(undefined, 'host-run', 'failed'),
      error: 'Core process restarted.'
    }
  }
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[running]}
      lifecycle
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  await expect.poll(() => view.refs.activeRunBindings.current.has('host-run')).toBe(true)

  vi.useFakeTimers()
  try {
    await screen.getByRole('button', { name: 'Reconcile stopped run' }).click()
    client.load.mockResolvedValueOnce(conversation('chat', [terminal]))
    for (const listener of client.resyncListeners) listener()
    await vi.waitFor(() => {
      expect(view.conversations[0].messages[0].agentRun?.status).toBe('failed')
    })

    client.load.mockRejectedValue(new Error('Storage temporarily unavailable'))
    await vi.advanceTimersByTimeAsync(20_000)
  } finally {
    vi.useRealTimers()
  }

  expect(view.conversations[0].messages[0].agentRun?.error).toBe('Core process restarted.')
  expect(client.toast).not.toHaveBeenCalled()
  expect(view.refs.activeRunBindings.current.has('host-run')).toBe(false)
  expect(view.refs.stopRequestedRunIds.current.has('host-run')).toBe(false)
  expect(view.refs.autoSubmitQueuedMessage.current).not.toHaveBeenCalledWith('chat')
  expect(client.start).not.toHaveBeenCalled()
  await screen.unmount()
})

it('does not resurrect an earlier approval when its pending-list reply arrives after the Run has reached sync input', async () => {
  const pending = deferred<PendingAgentActionSnapshot[]>()
  client.pendingActions.mockReturnValueOnce(pending.promise)
  let view!: HarnessView
  const screen = await render(
    <Harness
      initial={[conversation('chat', [assistant('waiting_for_approval')])]}
      lifecycle
      onRender={(next) => {
        view = next
      }}
    />
  )
  await expect.poll(() => client.pendingActions).toHaveBeenCalledTimes(1)
  client.load.mockResolvedValueOnce(conversation('chat', [assistant('waiting_for_user_input')]))
  notify()
  await expect
    .poll(() => view.conversations[0].messages[0].agentRun?.status)
    .toBe('waiting_for_user_input')
  pending.resolve([
    {
      actionId: '11111111-1111-4111-8111-111111111111',
      actionType: 'builtin_capability_activation',
      toolName: 'activate_capability',
      toolCallId: `tc1_${'a'.repeat(43)}`,
      runId: 'host-run',
      conversationId: 'chat',
      assistantMessageId: 'assistant',
      action: {
        type: 'builtin_capability_activation',
        approval: {
          actionId: '11111111-1111-4111-8111-111111111111',
          activationId: '22222222-2222-4222-8222-222222222222',
          runId: 'host-run',
          callId: `tc1_${'a'.repeat(43)}`,
          capabilityId: 'browser_automation',
          displayName: 'Browser automation',
          reason: 'Read the task page.',
          manifestDigest: `sha256:${'b'.repeat(64)}`,
          policyRevision: 7,
          createdAt: 1,
          expiresAt: 100,
          approvalStatus: 'required'
        }
      },
      createdAt: 1,
      status: 'pending'
    }
  ])
  // Let the list continuation and React commit both finish before checking the unchanged status.
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
  expect(view.conversations[0].messages[0].agentRun?.status).toBe('waiting_for_user_input')
  expect(view.conversations[0].messages[0].agentRun?.approvals).toEqual([])
  expect(client.start).not.toHaveBeenCalled()
  await screen.unmount()
})
