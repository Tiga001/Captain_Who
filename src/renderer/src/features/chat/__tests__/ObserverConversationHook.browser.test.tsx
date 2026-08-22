import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import {
  AGENT_COLLABORATION_SCHEMA_VERSION,
  type AgentObserverConversation
} from '@mycopilot/protocol'
import { useObserverConversation } from '../../agentCollaboration/useObserverConversation'

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  observerListeners: new Set<(event: unknown) => void>(),
  agentListeners: new Set<(event: unknown) => void>(),
  observerEvent: undefined as ((event: unknown) => void) | undefined,
  agentEvent: undefined as ((event: unknown) => void) | undefined
}))

vi.mock('../../agentCollaboration/collaborationClient', () => ({
  loadCollaborationObserverConversation: mocks.load,
  onCollaborationObserverEvent: vi.fn((handler) => {
    mocks.observerListeners.add(handler)
    mocks.observerEvent = (event) => {
      for (const listener of mocks.observerListeners) listener(event)
    }
    return () => {
      mocks.observerListeners.delete(handler)
    }
  })
}))

vi.mock('../../agent/agentClient', () => ({
  onAgentEvent: vi.fn((handler) => {
    mocks.agentListeners.add(handler)
    mocks.agentEvent = (event) => {
      for (const listener of mocks.agentListeners) listener(event)
    }
    return () => {
      mocks.agentListeners.delete(handler)
    }
  })
}))

beforeEach(() => {
  mocks.load.mockReset()
  mocks.observerListeners.clear()
  mocks.agentListeners.clear()
  mocks.observerEvent = undefined
  mocks.agentEvent = undefined
})

function observer(conversationId: string, content: string): AgentObserverConversation {
  return {
    schemaVersion: AGENT_COLLABORATION_SCHEMA_VERSION,
    agentId: `agent-${conversationId}`,
    rootConversationId: 'root-conversation',
    conversationId,
    projectId: null,
    modelId: 'child-model',
    title: conversationId,
    createdAt: 1,
    updatedAt: 2,
    messages: [
      {
        messageId: `message-${conversationId}`,
        role: 'assistant',
        content,
        createdAt: 2,
        status: 'sent',
        inputOrigin: null,
        attachments: [],
        agentRunJson: null,
        uiStateJson: null
      }
    ]
  }
}

function runningObserver(
  conversationId: string,
  runId: string,
  assistantMessageId: string,
  content = '正在思考...'
): AgentObserverConversation {
  const value = observer(conversationId, content)
  value.messages[0] = {
    ...value.messages[0]!,
    messageId: assistantMessageId,
    status: 'pending',
    agentRunJson: JSON.stringify({
      runId,
      status: 'running',
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      webSearchActivities: [],
      readActivities: [],
      approvals: [],
      diffs: [],
      fileDrafts: [],
      mcpInvocations: [],
      messageStreamCheckpoints: {},
      timeline: []
    })
  }
  return value
}

function HookProbe({
  conversationId,
  invalidationVersion = 0,
  rootAgentId = 'root-agent',
  rootConversationId = 'root-conversation'
}: {
  conversationId: string
  invalidationVersion?: number | string
  rootAgentId?: string
  rootConversationId?: string
}) {
  const state = useObserverConversation({
    agentId: `agent-${conversationId}`,
    conversationId,
    invalidationVersion,
    rootAgentId,
    rootConversationId
  })
  const commandOutput = state.conversation?.messages[0]?.agentRun?.commandOutputPreviews?.[
    'call-command'
  ]?.chunks
    .map((chunk) => chunk.output)
    .join('')
  return (
    <div
      data-command-output={commandOutput}
      data-conversation-id={state.conversation?.id}
      data-error={state.error ?? undefined}
      data-loading={state.loading ? 'true' : 'false'}
    >
      {state.conversation?.messages[0]?.content}
      {state.error ? <button aria-label="reload" onClick={state.reload} type="button" /> : null}
    </div>
  )
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}

it('rejects a stale child response after a rapid observer switch', async () => {
  const first = deferred<AgentObserverConversation | null>()
  const second = deferred<AgentObserverConversation | null>()
  mocks.load.mockReset().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)

  const screen = await render(<HookProbe conversationId="child-a" />)
  await screen.rerender(<HookProbe conversationId="child-b" />)
  second.resolve(observer('child-b', 'new child'))
  await expect.element(screen.getByText('new child')).toBeVisible()

  first.resolve(observer('child-a', 'stale child'))
  await new Promise((resolve) => window.setTimeout(resolve, 0))
  expect(screen.container.textContent).toBe('new child')
  expect(
    screen.container.querySelector('[data-conversation-id]')?.getAttribute('data-conversation-id')
  ).toBe('child-b')
})

it('clears the previous child synchronously while the next exact child is loading', async () => {
  const second = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(observer('child-a', 'old child history'))
    .mockReturnValueOnce(second.promise)

  const screen = await render(<HookProbe conversationId="child-a" />)
  await expect.element(screen.getByText('old child history')).toBeVisible()

  await screen.rerender(<HookProbe conversationId="child-b" />)
  expect(screen.container.textContent).not.toContain('old child history')
  expect(screen.container.querySelector('[data-loading]')?.getAttribute('data-loading')).toBe(
    'true'
  )

  second.resolve(observer('child-b', 'new child history'))
  await expect.element(screen.getByText('new child history')).toBeVisible()
})

it('reloads the exact observer snapshot when the external durable sequence advances', async () => {
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(observer('child-a', 'first snapshot'))
    .mockResolvedValueOnce(observer('child-a', 'updated snapshot'))

  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={10} />)
  await expect.element(screen.getByText('first snapshot')).toBeVisible()
  await screen.rerender(<HookProbe conversationId="child-a" invalidationVersion={11} />)
  await expect.element(screen.getByText('updated snapshot')).toBeVisible()

  expect(mocks.load).toHaveBeenNthCalledWith(1, {
    rootConversationId: 'root-conversation',
    conversationId: 'child-a'
  })
  expect(mocks.load).toHaveBeenNthCalledWith(2, {
    rootConversationId: 'root-conversation',
    conversationId: 'child-a'
  })
})

it('keeps the current child visible while a same-scope durable refresh is pending', async () => {
  const refreshed = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(observer('child-a', 'stable timeline'))
    .mockReturnValueOnce(refreshed.promise)

  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={10} />)
  await expect.element(screen.getByText('stable timeline')).toBeVisible()

  await screen.rerender(<HookProbe conversationId="child-a" invalidationVersion={11} />)
  expect(screen.container.textContent).toContain('stable timeline')

  refreshed.resolve(observer('child-a', 'new durable timeline'))
  await expect.element(screen.getByText('new durable timeline')).toBeVisible()
})

it('retains an authorized same-scope snapshot when refresh fails and recovers on retry', async () => {
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(observer('child-a', 'stable authorized timeline'))
    .mockRejectedValueOnce(new Error('temporary observer transport failure'))
    .mockResolvedValueOnce(observer('child-a', 'recovered durable timeline'))

  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={10} />)
  await expect.element(screen.getByText('stable authorized timeline')).toBeVisible()

  await screen.rerender(<HookProbe conversationId="child-a" invalidationVersion={11} />)
  await expect.element(screen.getByRole('button', { name: 'reload' })).toBeVisible()
  expect(screen.container.textContent).toContain('stable authorized timeline')
  expect(screen.container.querySelector('[data-error]')?.getAttribute('data-error')).toBe(
    'temporary observer transport failure'
  )

  await screen.getByRole('button', { name: 'reload' }).click()
  await expect.element(screen.getByText('recovered durable timeline')).toBeVisible()
  expect(screen.container.textContent).not.toContain('stable authorized timeline')
})

it('fails closed when exact observer identity changes even if the conversation id is unchanged', async () => {
  const failedIdentityLoad = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(observer('child-a', 'old exact identity'))
    .mockReturnValueOnce(failedIdentityLoad.promise)

  const screen = await render(<HookProbe conversationId="child-a" rootAgentId="root-agent-a" />)
  await expect.element(screen.getByText('old exact identity')).toBeVisible()

  await screen.rerender(<HookProbe conversationId="child-a" rootAgentId="root-agent-b" />)
  expect(screen.container.textContent).not.toContain('old exact identity')
  expect(screen.container.querySelector('[data-loading]')?.getAttribute('data-loading')).toBe(
    'true'
  )

  failedIdentityLoad.resolve(null)
  await expect.element(screen.getByRole('button', { name: 'reload' })).toBeVisible()
  expect(screen.container.textContent).not.toContain('old exact identity')
})

it('streams only an exact root/agent/conversation/run/message observer envelope', async () => {
  mocks.load
    .mockReset()
    .mockResolvedValue(runningObserver('child-a', 'run-shared', 'assistant-a', 'initial child'))
  const screen = await render(<HookProbe conversationId="child-a" />)
  await expect.element(screen.getByText('initial child')).toBeVisible()

  const base = {
    schemaVersion: 1 as const,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-shared',
    assistantMessageId: 'assistant-a'
  }
  mocks.observerEvent?.({
    ...base,
    agentId: 'agent-child-b',
    event: { type: 'message_delta', runId: 'run-shared', delta: ' forged agent' }
  })
  mocks.observerEvent?.({
    ...base,
    rootAgentId: 'foreign-root-agent',
    event: { type: 'message_delta', runId: 'run-shared', delta: ' forged root agent' }
  })
  mocks.observerEvent?.({
    ...base,
    conversationId: 'child-b',
    event: { type: 'message_delta', runId: 'run-shared', delta: ' forged conversation' }
  })
  await new Promise((resolve) => window.setTimeout(resolve, 0))
  expect(screen.container.textContent).not.toContain('forged')

  mocks.observerEvent?.({
    ...base,
    event: { type: 'message_delta', runId: 'run-shared', delta: ' + live' }
  })
  await expect.element(screen.getByText('initial child + live')).toBeVisible()
})

it('rejects late child events after a rapid switch and lets a durable reload replace live state', async () => {
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(runningObserver('child-a', 'run-a', 'assistant-a', 'child A'))
    .mockResolvedValueOnce(runningObserver('child-b', 'run-b', 'assistant-b', 'child B'))
    .mockResolvedValueOnce(runningObserver('child-b', 'run-b', 'assistant-b', 'durable B'))
  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={1} />)
  await expect.element(screen.getByText('child A')).toBeVisible()
  await screen.rerender(<HookProbe conversationId="child-b" invalidationVersion={1} />)
  await expect.element(screen.getByText('child B')).toBeVisible()

  mocks.observerEvent?.({
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-a',
    assistantMessageId: 'assistant-a',
    event: { type: 'message_delta', runId: 'run-a', delta: ' late A' }
  })
  mocks.observerEvent?.({
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-b',
    conversationId: 'child-b',
    runId: 'run-b',
    assistantMessageId: 'assistant-b',
    event: { type: 'message_delta', runId: 'run-b', delta: ' live B' }
  })
  await expect.element(screen.getByText('child B live B')).toBeVisible()
  expect(screen.container.textContent).not.toContain('late A')

  await screen.rerender(<HookProbe conversationId="child-b" invalidationVersion={2} />)
  await expect.element(screen.getByText('durable B')).toBeVisible()
  expect(screen.container.textContent).not.toContain('live B')
})

it('replays an event that arrives while the initial authorized snapshot is loading', async () => {
  const pending = deferred<AgentObserverConversation | null>()
  mocks.load.mockReset().mockReturnValue(pending.promise)
  const screen = await render(<HookProbe conversationId="child-a" />)
  mocks.observerEvent?.({
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-a',
    assistantMessageId: 'assistant-a',
    event: { type: 'message_delta', runId: 'run-a', delta: ' during load' }
  })
  pending.resolve(runningObserver('child-a', 'run-a', 'assistant-a', 'snapshot'))
  await expect.element(screen.getByText('snapshot during load')).toBeVisible()
})

it('replays a live delta that races a same-scope durable refresh', async () => {
  const refresh = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(runningObserver('child-a', 'run-a', 'assistant-a', 'first'))
    .mockReturnValueOnce(refresh.promise)
  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={1} />)
  await expect.element(screen.getByText('first')).toBeVisible()
  await screen.rerender(<HookProbe conversationId="child-a" invalidationVersion={2} />)
  mocks.observerEvent?.({
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-a',
    assistantMessageId: 'assistant-a',
    event: { type: 'message_delta', runId: 'run-a', delta: ' raced' }
  })
  refresh.resolve(runningObserver('child-a', 'run-a', 'assistant-a', 'durable'))
  await expect.element(screen.getByText('durable raced')).toBeVisible()
})

it('lets a target durable refresh authoritatively replace a pre-cut live delta', async () => {
  const refresh = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReset()
    .mockResolvedValueOnce(runningObserver('child-a', 'run-a', 'assistant-a', 'base'))
    .mockReturnValueOnce(refresh.promise)
  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion={1} />)
  await expect.element(screen.getByText('base')).toBeVisible()
  mocks.observerEvent?.({
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-a',
    assistantMessageId: 'assistant-a',
    event: { type: 'message_delta', runId: 'run-a', delta: ' before refresh' }
  })
  await expect.element(screen.getByText('base before refresh')).toBeVisible()
  await screen.rerender(<HookProbe conversationId="child-a" invalidationVersion={2} />)
  refresh.resolve(runningObserver('child-a', 'run-a', 'assistant-a', 'base'))
  await expect
    .poll(() => screen.container.querySelector('[data-loading]')?.getAttribute('data-loading'))
    .toBe('false')
  expect(screen.container.textContent).not.toContain('before refresh')
})

it('keeps legacy Command output isolated by exact conversation, message and run identity', async () => {
  const value = runningObserver('child-a', 'run-command', 'assistant-command', 'working')
  const stored = JSON.parse(value.messages[0]!.agentRunJson!)
  stored.toolCalls = [
    {
      id: 'call-command',
      tool: 'run_command',
      args: { command: 'echo hi' },
      approvalStatus: 'approved',
      reason: null
    }
  ]
  stored.timeline = [{ id: 'tool-call-call-command', type: 'tool_call', callId: 'call-command' }]
  value.messages[0]!.agentRunJson = JSON.stringify(stored)
  mocks.load.mockReset().mockResolvedValue(value)
  const screen = await render(<HookProbe conversationId="child-a" />)
  await expect.element(screen.getByText('working')).toBeVisible()

  mocks.agentEvent?.({
    type: 'command_output',
    runId: 'run-command',
    conversationId: 'child-b',
    assistantMessageId: 'assistant-command',
    callId: 'call-command',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence: 1,
    stream: 'stdout',
    output: 'wrong child'
  })
  mocks.agentEvent?.({
    type: 'command_output',
    runId: 'run-command',
    conversationId: 'child-a',
    assistantMessageId: 'assistant-command',
    callId: 'call-command',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence: 1,
    stream: 'stdout',
    output: 'right child'
  })
  mocks.agentEvent?.({
    type: 'command_output',
    runId: 'run-command',
    conversationId: 'child-a',
    assistantMessageId: 'assistant-command',
    callId: 'call-command',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence: 1,
    stream: 'stdout',
    output: 'duplicate should be ignored'
  })
  await expect
    .poll(() =>
      screen.container.querySelector('[data-command-output]')?.getAttribute('data-command-output')
    )
    .toBe('right child')
  expect(screen.container.textContent).not.toContain('wrong child')
  expect(
    screen.container.querySelector('[data-command-output]')?.getAttribute('data-command-output')
  ).toBe('right child')
})

it('keeps two mounted observers with the same run id isolated and independently subscribed', async () => {
  mocks.load
    .mockReset()
    .mockImplementation(
      async ({
        conversationId,
        rootConversationId
      }: {
        conversationId: string
        rootConversationId: string
      }) => {
        const value = runningObserver(
          conversationId,
          'shared-run',
          `assistant-${conversationId}`,
          `initial-${conversationId}`
        )
        value.rootConversationId = rootConversationId
        value.agentId = `agent-${conversationId}`
        return value
      }
    )
  const screen = await render(
    <>
      <HookProbe conversationId="child-a" rootAgentId="root-agent-a" rootConversationId="root-a" />
      <HookProbe conversationId="child-b" rootAgentId="root-agent-b" rootConversationId="root-b" />
    </>
  )
  await expect.element(screen.getByText('initial-child-a')).toBeVisible()
  await expect.element(screen.getByText('initial-child-b')).toBeVisible()

  for (const [rootAgentId, rootConversationId, conversationId, delta] of [
    ['root-agent-a', 'root-a', 'child-a', ' +A'],
    ['root-agent-b', 'root-b', 'child-b', ' +B']
  ] as const) {
    mocks.observerEvent?.({
      schemaVersion: 1,
      rootAgentId,
      rootConversationId,
      agentId: `agent-${conversationId}`,
      conversationId,
      runId: 'shared-run',
      assistantMessageId: `assistant-${conversationId}`,
      event: { type: 'message_delta', runId: 'shared-run', delta }
    })
  }

  await expect.element(screen.getByText('initial-child-a +A')).toBeVisible()
  await expect.element(screen.getByText('initial-child-b +B')).toBeVisible()
  expect(mocks.observerListeners.size).toBe(2)
})

it('does not journal or reload a long stream after the durable snapshot has loaded', async () => {
  mocks.load.mockResolvedValue(runningObserver('child-a', 'run-a', 'assistant-a', 'base'))
  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion="1:1" />)
  await expect.element(screen.getByText('base')).toBeVisible()

  const delta = 'x'
  for (let index = 0; index < 520; index += 1) {
    mocks.observerEvent?.({
      schemaVersion: 1,
      rootAgentId: 'root-agent',
      rootConversationId: 'root-conversation',
      agentId: 'agent-child-a',
      conversationId: 'child-a',
      runId: 'run-a',
      assistantMessageId: 'assistant-a',
      event: { type: 'message_delta', runId: 'run-a', delta }
    })
  }

  await expect.poll(() => screen.container.textContent?.length).toBe('base'.length + 520)
  expect(mocks.load).toHaveBeenCalledTimes(1)
})

it('rejects an overflowed in-flight snapshot until an explicit durable reload succeeds', async () => {
  const initial = deferred<AgentObserverConversation | null>()
  mocks.load
    .mockReturnValueOnce(initial.promise)
    .mockResolvedValueOnce(
      runningObserver('child-a', 'run-a', 'assistant-a', `durable${'x'.repeat(520)}`)
    )
  const screen = await render(<HookProbe conversationId="child-a" invalidationVersion="1:1" />)

  for (let index = 0; index < 520; index += 1) {
    mocks.observerEvent?.({
      schemaVersion: 1,
      rootAgentId: 'root-agent',
      rootConversationId: 'root-conversation',
      agentId: 'agent-child-a',
      conversationId: 'child-a',
      runId: 'run-a',
      assistantMessageId: 'assistant-a',
      event: { type: 'message_delta', runId: 'run-a', delta: 'x' }
    })
  }
  initial.resolve(runningObserver('child-a', 'run-a', 'assistant-a', 'stale'))
  await expect.element(screen.getByRole('button', { name: 'reload' })).toBeVisible()
  expect(screen.container.textContent).not.toContain('stale')
  expect(screen.container.querySelector('[data-loading]')?.getAttribute('data-loading')).toBe(
    'false'
  )
  expect(screen.container.querySelector('[data-error]')?.getAttribute('data-error')).toContain(
    'Retry to recover'
  )
  expect(mocks.load).toHaveBeenCalledTimes(1)

  await screen.getByRole('button', { name: 'reload' }).click()
  await expect.poll(() => screen.container.textContent?.length).toBe('durable'.length + 520)
  expect(mocks.load).toHaveBeenCalledTimes(2)
})

it('resets an obsolete model attempt before streaming the retry through the shared reducer', async () => {
  mocks.load.mockResolvedValue(runningObserver('child-a', 'run-a', 'assistant-a', 'base'))
  const screen = await render(<HookProbe conversationId="child-a" />)
  await expect.element(screen.getByText('base')).toBeVisible()
  const identity = {
    schemaVersion: 1 as const,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'agent-child-a',
    conversationId: 'child-a',
    runId: 'run-a',
    assistantMessageId: 'assistant-a'
  }
  for (const event of [
    { type: 'message_stream_started', runId: 'run-a', streamId: 'stream-1', attempt: 1 },
    { type: 'message_delta', runId: 'run-a', streamId: 'stream-1', delta: ' obsolete' },
    {
      type: 'message_stream_reset',
      runId: 'run-a',
      streamId: 'stream-1',
      reason: 'retrying_model_request'
    },
    {
      type: 'llm_retry',
      runId: 'run-a',
      streamId: 'stream-1',
      category: 'overloaded',
      delayMs: 0,
      retryAt: 1,
      attempt: 1,
      maxAttempts: 2
    },
    { type: 'message_stream_started', runId: 'run-a', streamId: 'stream-1', attempt: 2 },
    { type: 'message_delta', runId: 'run-a', streamId: 'stream-1', delta: ' fresh' }
  ]) {
    mocks.observerEvent?.({ ...identity, event })
  }

  await expect.element(screen.getByText(' fresh')).toBeVisible()
  expect(screen.container.textContent).not.toContain('obsolete')
})
