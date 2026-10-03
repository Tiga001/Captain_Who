import { act } from 'react'
import { createWorkflow } from '../../features/workflows/workflowAuthoring'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  AgentEvent,
  CollaborationEventEnvelope,
  WorkflowInstance,
  WorkflowResponse
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useWorkflowActivity } from '../../features/workflows/project/useWorkflowActivity'

const mocks = vi.hoisted(() => ({
  request: vi.fn(),
  agents: new Set<(event: AgentEvent) => void>(),
  collaboration: new Set<(event: CollaborationEventEnvelope) => void>(),
  resync: new Set<() => void>()
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onEvent: (listener: (event: AgentEvent) => void) => {
        mocks.agents.add(listener)
        return () => mocks.agents.delete(listener)
      },
      onCollaborationEvent: (listener: (event: CollaborationEventEnvelope) => void) => {
        mocks.collaboration.add(listener)
        return () => mocks.collaboration.delete(listener)
      },
      onCollaborationResync: (listener: () => void) => {
        mocks.resync.add(listener)
        return () => mocks.resync.delete(listener)
      }
    }
  }
}))

const workflow = (patch: Partial<WorkflowInstance> = {}): WorkflowInstance => ({
  id: 'workflow-a',
  templateId: 'template-a',
  templateRevision: 1,
  definition: createWorkflow(),
  name: 'Review',
  color: '#2478d4',
  revision: 1,
  updatedAt: 1,
  needsReview: false,
  enabled: true,
  running: false,
  bindings: [{ nodeId: 'agent-a', conversationId: 'chat-a' }],
  ...patch
})
const conversation = (pending = false): ChatConversation => ({
  id: 'chat-a',
  projectId: null,
  modelId: 'model',
  title: 'Task',
  createdAt: 1,
  updatedAt: 1,
  messages: pending
    ? [{ id: 'reply', role: 'assistant', content: '', createdAt: 1, status: 'pending' }]
    : [],
  messagesLoaded: pending
})
const result = (instances: WorkflowInstance[]): WorkflowResponse => ({
  records: [],
  issues: [],
  instances
})
const emitAgent = (event: AgentEvent) => mocks.agents.forEach((listener) => listener(event))
const emitCollaboration = (root: string, kind: CollaborationEventEnvelope['kind']) =>
  mocks.collaboration.forEach((listener) =>
    listener({
      schemaVersion: 3,
      eventId: 'event',
      sequence: 1,
      workspaceId: null,
      projectId: null,
      rootAgentId: 'root',
      rootConversationId: root,
      agentId: 'child',
      conversationId: 'child-chat',
      turnId: 'turn',
      runId: 'run',
      messageId: null,
      kind,
      resourceRevision: 1,
      activities: [],
      occurredAt: 1
    })
  )

function Harness({
  instances,
  conversations = []
}: {
  instances: WorkflowInstance[]
  conversations?: ChatConversation[]
}) {
  const { runningInstanceIds, activityByInstanceId } = useWorkflowActivity(instances, conversations)
  return (
    <>
      <output data-testid="activity">{[...runningInstanceIds].sort().join(',') || 'idle'}</output>
      <output data-testid="intervals">{JSON.stringify([...activityByInstanceId])}</output>
    </>
  )
}
const advance = async (ms = 100) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

beforeEach(() => {
  vi.useFakeTimers({
    toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date']
  })
  mocks.request.mockReset().mockResolvedValue(result([workflow()]))
  mocks.agents.clear()
  mocks.collaboration.clear()
  mocks.resync.clear()
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('organization live activity', () => {
  it('shows a local root run immediately, stops after settlement, and never lights disabled organizations', async () => {
    const instances = [workflow(), workflow({ id: 'disabled', enabled: false, running: true })]
    const view = await render(
      <Harness instances={instances} conversations={[conversation(true)]} />
    )
    await expect.element(view.getByTestId('activity')).toHaveTextContent('workflow-a')
    expect(view.getByTestId('intervals').element().textContent).toBe(
      JSON.stringify([
        ['workflow-a', null],
        ['disabled', null]
      ])
    )
    await advance()
    await view.rerender(<Harness instances={instances} conversations={[conversation()]} />)
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
  })

  it('keeps durable interval boundaries through failed reads, settlement, a new segment and remount', async () => {
    const first = { startedAt: 1_000, completedAt: null }
    const initial = [workflow({ running: true, activity: first })]
    mocks.request.mockResolvedValue(result(initial))
    const view = await render(<Harness instances={initial} />)
    await advance()
    const interval = () => JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]
    expect(interval()).toEqual(first)
    mocks.request.mockRejectedValueOnce(new Error('offline'))
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    expect(interval()).toEqual(first)

    const ended = { startedAt: 1_000, completedAt: 9_000 }
    mocks.request.mockResolvedValue(result([workflow({ activity: ended })]))
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    expect(interval()).toEqual(ended)
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
    await advance(10_000)
    expect(interval()).toEqual(ended)

    const next = { startedAt: 30_000, completedAt: null }
    const latest = [workflow({ running: true, activity: next })]
    mocks.request.mockResolvedValue(result(latest))
    emitCollaboration('chat-a', 'turn_started')
    await advance()
    expect(interval()).toEqual(next)
    await view.unmount()
    const remounted = await render(<Harness instances={latest} />)
    expect(JSON.parse(remounted.getByTestId('intervals').element().textContent!)[0][1]).toEqual(
      next
    )
  })

  it('accepts a newer parent interval without a configuration revision change', async () => {
    const initial = [workflow({ running: true, activity: { startedAt: 1_000, completedAt: null } })]
    mocks.request.mockResolvedValue(result(initial))
    const view = await render(<Harness instances={initial} />)
    await advance()
    const ended = { startedAt: 1_000, completedAt: 5_000 }
    await view.rerender(<Harness instances={[workflow({ activity: ended })]} />)
    // A parent snapshot must not freeze a newer background read of this same interval.
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toEqual(
      initial[0].activity
    )
    const next = { startedAt: 8_000, completedAt: null }
    await view.rerender(<Harness instances={[workflow({ activity: next })]} />)
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toEqual(next)
  })

  it('honors an authoritative null activity instead of restoring stale parent timestamps', async () => {
    const initial = [workflow({ activity: { startedAt: 1_000, completedAt: null } })]
    mocks.request.mockResolvedValue(result([workflow({ activity: null })]))
    const view = await render(<Harness instances={initial} />)
    await advance()
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toBeNull()
    mocks.request.mockRejectedValue(new Error('offline'))
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toBeNull()
  })

  it('continues refreshing a disabled organization until its already active interval settles', async () => {
    const open = workflow({ enabled: false, activity: { startedAt: 1_000, completedAt: null } })
    mocks.request.mockResolvedValue(result([open]))
    const view = await render(<Harness instances={[open]} />)
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
    expect(mocks.agents.size).toBe(1)
    const ended = { startedAt: 1_000, completedAt: 9_000 }
    mocks.request.mockResolvedValue(result([{ ...open, activity: ended }]))
    emitCollaboration('chat-a', 'turn_updated')
    await advance(1_000)
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toEqual(ended)
    expect(mocks.agents.size).toBe(0)
    mocks.request.mockClear()
    await advance(10_000)
    expect(mocks.request).not.toHaveBeenCalled()
  })

  it('settles an open interval after all current member bindings have been removed', async () => {
    const open = workflow({
      enabled: false,
      bindings: [],
      activity: { startedAt: 1_000, completedAt: null }
    })
    mocks.request.mockResolvedValue(result([open]))
    const view = await render(<Harness instances={[open]} />)
    await advance()
    expect(mocks.agents.size).toBe(1)
    const ended = { startedAt: 1_000, completedAt: 8_000 }
    mocks.request.mockResolvedValue(result([{ ...open, activity: ended }]))
    // Recovery does not depend on the removed conversations still appearing in the binding set.
    await advance(5_000)
    expect(JSON.parse(view.getByTestId('intervals').element().textContent!)[0][1]).toEqual(ended)
    expect(mocks.agents.size).toBe(0)
    mocks.request.mockClear()
    await advance(10_000)
    expect(mocks.request).not.toHaveBeenCalled()
  })

  it('tracks metadata-only and child turns, replacing an initially running snapshot when all turns settle', async () => {
    const instances = [workflow({ running: true })]
    mocks.request.mockResolvedValue(result(instances))
    const view = await render(<Harness instances={instances} conversations={[conversation()]} />)
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('workflow-a')
    mocks.request.mockResolvedValue(result([workflow({ running: false })]))
    emitCollaboration('chat-a', 'turn_updated')
    await advance(1_000)
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
    mocks.request.mockResolvedValue(result([workflow({ running: true })]))
    emitCollaboration('chat-a', 'turn_started')
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('workflow-a')
  })

  it('coalesces lifecycle reads and ignores token streams and unrelated collaboration trees', async () => {
    await render(<Harness instances={[workflow()]} />)
    await advance()
    mocks.request.mockClear()
    for (let i = 0; i < 300; i++) emitAgent({ type: 'message_delta', runId: 'run', delta: 'x' })
    emitCollaboration('unrelated-root', 'turn_updated')
    await advance()
    expect(mocks.request).not.toHaveBeenCalled()
    emitAgent({ type: 'started', runId: 'run', toolDefinitions: [] })
    emitCollaboration('chat-a', 'turn_started')
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    expect(mocks.request).toHaveBeenCalledExactlyOnceWith({ operation: 'listInstances' })
    mocks.request.mockClear()
    for (let i = 0; i < 10; i++) {
      emitCollaboration('chat-a', 'turn_updated')
      await advance(50)
    }
    expect(mocks.request).not.toHaveBeenCalled()
    // A final lifecycle event expedites a pending progress read instead of waiting for its timer.
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    expect(mocks.request).toHaveBeenCalledTimes(1)
  })

  it('reconciles events received during an outstanding read and ignores responses from a previous binding revision', async () => {
    const view = await render(<Harness instances={[workflow()]} />)
    await advance()
    let resolve!: (value: WorkflowResponse) => void
    mocks.request.mockImplementationOnce(
      () =>
        new Promise<WorkflowResponse>((done) => {
          resolve = done
        })
    )
    emitAgent({ type: 'started', runId: 'run', toolDefinitions: [] })
    await advance()
    emitAgent({ type: 'done', runId: 'run', success: true })
    await advance()
    mocks.request.mockResolvedValue(result([workflow({ running: false })]))
    await act(async () => {
      resolve(result([workflow({ running: true })]))
    })
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')

    mocks.request.mockImplementationOnce(
      () =>
        new Promise<WorkflowResponse>((done) => {
          resolve = done
        })
    )
    emitAgent({ type: 'started', runId: 'next-run', toolDefinitions: [] })
    await advance()
    await view.rerender(<Harness instances={[workflow({ revision: 2, enabled: false })]} />)
    await act(async () => {
      resolve(result([workflow({ running: true })]))
    })
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
    expect(mocks.agents.size).toBe(0)
    expect(mocks.collaboration.size).toBe(0)
  })

  it('retains activity on failed reads, recovers on resync and visibility, and stops listeners and polling on unmount', async () => {
    const view = await render(<Harness instances={[workflow({ running: true })]} />)
    mocks.request.mockRejectedValue(new Error('offline'))
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('workflow-a')
    mocks.request.mockResolvedValue(result([workflow({ running: false })]))
    mocks.resync.forEach((listener) => listener())
    await advance()
    await expect.element(view.getByTestId('activity')).toHaveTextContent('idle')
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden')
    mocks.request.mockClear()
    await advance(10_000)
    expect(mocks.request).not.toHaveBeenCalled()
    visibility.mockReturnValue('visible')
    document.dispatchEvent(new Event('visibilitychange'))
    await advance()
    expect(mocks.request).toHaveBeenCalledTimes(1)
    mocks.request.mockClear()
    await advance(5_100)
    expect(mocks.request).toHaveBeenCalledTimes(1)
    await view.unmount()
    mocks.request.mockClear()
    expect(mocks.agents.size).toBe(0)
    expect(mocks.collaboration.size).toBe(0)
    expect(mocks.resync.size).toBe(0)
    await advance(10_000)
    expect(mocks.request).not.toHaveBeenCalled()
  })
})
