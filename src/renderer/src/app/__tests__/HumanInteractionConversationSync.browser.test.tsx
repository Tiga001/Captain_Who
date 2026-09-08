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
  useAgentRunLifecycle({
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
    showToast: vi.fn(),
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
  return null
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
  client.pendingActions.mockReset().mockResolvedValue([])
  client.requestListeners.clear()
  client.resyncListeners.clear()
  client.agentListeners.clear()
})

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

it('refreshes loaded chats on Core reconnect and the active chat on focus without hydrating sidebar-only chats', async () => {
  const loaded = conversation(),
    other = conversation('other'),
    unloaded = { ...conversation('sidebar'), messagesLoaded: false }
  const screen = await render(<Harness initial={[loaded, other, unloaded]} onRender={() => {}} />)
  await expect.poll(() => client.load).toHaveBeenCalledTimes(1)
  client.load.mockClear()
  for (const listener of client.resyncListeners) listener()
  await expect.poll(() => client.load).toHaveBeenCalledTimes(2)
  expect(client.load.mock.calls.map(([id]) => id).sort()).toEqual(['chat', 'other'])
  client.load.mockClear()
  window.dispatchEvent(new Event('focus'))
  await expect.poll(() => client.load).toHaveBeenCalledExactlyOnceWith('chat')
  await screen.unmount()
  expect(client.requestListeners.size).toBe(0)
  expect(client.resyncListeners.size).toBe(0)
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
