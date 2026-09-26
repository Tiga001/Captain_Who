import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  AgentEvent,
  AgentStateSnapshot,
  AgentTreeSnapshot,
  CollaborationEventEnvelope,
  WorkflowInstance
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useWorkflowMonitor } from '../../features/workflows/project/useWorkflowMonitor'

const mocks = vi.hoisted(() => ({
  getTree: vi.fn(),
  agents: new Set<(event: AgentEvent) => void>(),
  collaboration: new Set<(event: CollaborationEventEnvelope) => void>(),
  resync: new Set<() => void>()
}))
vi.mock('../../features/agentCollaboration/collaborationClient', () => ({
  hostCollaborationDataSource: {
    getTree: mocks.getTree,
    subscribe: (listener: (event: CollaborationEventEnvelope) => void) => {
      mocks.collaboration.add(listener)
      return () => mocks.collaboration.delete(listener)
    },
    subscribeResync: (listener: () => void) => {
      mocks.resync.add(listener)
      return () => mocks.resync.delete(listener)
    },
    subscribeAgentEvents: (listener: (event: AgentEvent) => void) => {
      mocks.agents.add(listener)
      return () => mocks.agents.delete(listener)
    }
  }
}))

const workflow = (patch: Partial<WorkflowInstance> = {}): WorkflowInstance => ({
  id: 'workflow',
  templateId: 'template',
  templateRevision: 1,
  name: 'Review',
  color: '#2478d4',
  revision: 1,
  updatedAt: 1,
  needsReview: false,
  enabled: true,
  running: false,
  bindings: [
    { nodeId: 'a', conversationId: 'chat-a' },
    { nodeId: 'b', conversationId: 'chat-b' }
  ],
  ...patch
})
const conversation = (id: string, pending = false): ChatConversation => ({
  id,
  projectId: null,
  modelId: 'model',
  title: id,
  createdAt: 1,
  updatedAt: 1,
  messages: pending
    ? [{ id: 'reply', role: 'assistant', content: '', createdAt: 1, status: 'pending' }]
    : [],
  messagesLoaded: pending
})
function tree(
  root: string,
  active: string[] = [],
  sequence = 0,
  waitingApproval: string[] = []
): AgentTreeSnapshot {
  return {
    schemaVersion: 1,
    workspaceId: null,
    projectId: null,
    rootAgentId: root,
    rootConversationId: root,
    lastSequence: sequence,
    agents: [root, `${root}-child`].map((id, index) => ({
      agentId: id,
      rootAgentId: root,
      rootConversationId: root,
      parentAgentId: index === 0 ? null : root,
      conversationId: id,
      projectId: null,
      taskName: id,
      taskPath: `/${id}`,
      lifecycle: 'active',
      displayStatus: waitingApproval.includes(id)
        ? 'waiting_approval'
        : active.includes(id)
          ? 'running'
          : 'idle',
      latestActivityAt: 1,
      model: null
    }))
  }
}
function event(
  sequence: number,
  patch: Partial<CollaborationEventEnvelope> = {}
): CollaborationEventEnvelope {
  return {
    schemaVersion: 3,
    eventId: `event-${sequence}`,
    sequence,
    workspaceId: null,
    projectId: null,
    rootAgentId: 'chat-a',
    rootConversationId: 'chat-a',
    agentId: 'chat-a',
    conversationId: 'chat-a',
    turnId: 'turn',
    runId: 'run',
    messageId: null,
    kind: 'turn_started',
    resourceRevision: 1,
    activities: [],
    occurredAt: 1,
    ...patch
  }
}
const emit = (value: CollaborationEventEnvelope) =>
  act(() => mocks.collaboration.forEach((fn) => fn(value)))
const emitAgent = (value: AgentEvent) => act(() => mocks.agents.forEach((fn) => fn(value)))
const runState = (status: AgentStateSnapshot['status'], runId = 'run'): AgentEvent => ({
  type: 'state',
  runId,
  state: { status, activeRunId: runId, lastError: null, updatedAt: 1 }
})
const approvalActivity = (agentId: string): CollaborationEventEnvelope['activities'][number] => ({
  schemaVersion: 4,
  activityId: `approval-${agentId}`,
  semantic: 'waiting_approval',
  agentId,
  taskNameSnapshot: agentId,
  ownerAgentId: 'chat-a',
  ownerConversationId: 'chat-a',
  taskMessageId: null,
  anchorMessageId: null,
  traceBoundarySequence: null
})
const advance = async (ms = 100) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}
function Harness({
  instance = workflow(),
  conversations = []
}: {
  instance?: WorkflowInstance
  conversations?: ChatConversation[]
}) {
  const { runningConversationIds, waitingApprovalConversationIds } = useWorkflowMonitor(
    instance,
    conversations
  )
  return (
    <>
      <output data-testid="running">
        {[...runningConversationIds].sort().join(',') || 'idle'}
      </output>
      <output data-testid="waiting-approval">
        {[...waitingApprovalConversationIds].sort().join(',') || 'none'}
      </output>
    </>
  )
}

beforeEach(() => {
  vi.useFakeTimers({
    toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date']
  })
  mocks.getTree
    .mockReset()
    .mockImplementation(async ({ rootConversationId }) => tree(rootConversationId))
  mocks.agents.clear()
  mocks.collaboration.clear()
  mocks.resync.clear()
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('workflow read-only activity monitor', () => {
  it('aggregates root and child approvals from authoritative trees even for unloaded and disabled conversations', async () => {
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, [], 0, [rootConversationId === 'chat-a' ? 'chat-a' : 'chat-b-child'])
    )
    const view = await render(
      <Harness
        instance={workflow({ enabled: false, running: false })}
        conversations={[conversation('chat-a'), conversation('chat-b')]}
      />
    )
    await advance()
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a,chat-b')
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a,chat-b')
    mocks.getTree.mockImplementation(async ({ rootConversationId }) => {
      const snapshot = tree(rootConversationId, [], 0, [`${rootConversationId}-child`])
      snapshot.agents[1].lifecycle = 'archived'
      return snapshot
    })
    mocks.resync.forEach((listener) => listener())
    await advance()
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
  })

  it('clears approval indicators on actual resume and all terminal outcomes, while preserving other pending agents', async () => {
    const view = await render(<Harness />)
    await advance()
    emit(event(1))
    emit(
      event(2, {
        kind: 'approval_projected',
        activities: [approvalActivity('chat-a')]
      })
    )
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
    emitAgent(runState('running'))
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    let sequence = 3
    for (const status of ['completed', 'failed', 'cancelled'] as const) {
      const runId = `run-${status}`
      emit(event(sequence++, { runId }))
      emitAgent(runState('waiting_for_approval', runId))
      await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
      emitAgent({ type: 'done', runId, status, success: status === 'completed' })
      await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
    }
    emit(event(sequence++, { runId: 'root-run' }))
    emitAgent(runState('waiting_for_approval', 'root-run'))
    emit(event(sequence++, { agentId: 'chat-a-child', runId: 'child-run' }))
    emitAgent(runState('waiting_for_approval', 'child-run'))
    emitAgent({ type: 'done', runId: 'root-run', success: true })
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
    emitAgent({ type: 'done', runId: 'child-run', success: true })
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
  })

  it('uses approval updates to refresh metadata-only roots, and does not classify user input as approval', async () => {
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, [], 0, rootConversationId === 'chat-a' ? ['chat-a-child'] : [])
    )
    const view = await render(<Harness />)
    await advance()
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, ['chat-a-child'], 1)
    )
    emit(event(1, { kind: 'approval_updated', agentId: 'chat-a-child' }))
    await advance()
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    emitAgent(runState('waiting_for_user_input'))
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
  })

  it('ignores late completion and status updates from an older run while the newer run waits for approval', async () => {
    const view = await render(<Harness />)
    await advance()
    emit(event(1, { runId: 'older-run' }))
    emit(event(2, { runId: 'newer-run' }))
    emitAgent(runState('waiting_for_approval', 'newer-run'))
    emit(
      event(3, {
        kind: 'turn_updated',
        runId: 'older-run',
        transmission: {
          id: 'late-completion',
          kind: 'completion',
          sourceAgentId: 'chat-a',
          targetAgentId: null
        },
        activities: [{ ...approvalActivity('chat-a'), semantic: 'completed' }]
      })
    )
    emitAgent(runState('running', 'older-run'))
    emitAgent({ type: 'done', runId: 'older-run', success: true })
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    emitAgent({ type: 'done', runId: 'newer-run', success: true })
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
  })

  it('does not let older loaded messages resolve the approval state of the latest turn after opening the monitor', async () => {
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, [], 0, rootConversationId === 'chat-a' ? ['chat-a'] : [])
    )
    const chat = conversation('chat-a')
    chat.messages = ['old', 'current'].map((runId) => ({
      id: runId,
      role: 'assistant',
      content: '',
      createdAt: 1,
      agentRun: {
        runId,
        status: runId === 'current' ? 'waiting_for_approval' : 'completed',
        approvals: [],
        fileChangeProposals: [],
        toolDefinitions: [],
        toolCalls: [],
        toolResults: [],
        timeline: []
      }
    }))
    const view = await render(<Harness conversations={[chat]} />)
    await advance()
    emitAgent({ type: 'done', runId: 'old', success: true })
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('chat-a')
    emitAgent(runState('running', 'current'))
    await expect.element(view.getByTestId('waiting-approval')).toHaveTextContent('none')
  })

  it('reads each metadata-only root and its children independently, including disabled workflows', async () => {
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, rootConversationId === 'chat-a' ? ['chat-a-child'] : [])
    )
    const view = await render(
      <Harness
        instance={workflow({ enabled: false, running: true })}
        conversations={[conversation('chat-a'), conversation('chat-b')]}
      />
    )
    await advance()
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    expect(mocks.getTree).toHaveBeenCalledWith({ rootConversationId: 'chat-a' })
    expect(mocks.getTree).toHaveBeenCalledWith({ rootConversationId: 'chat-b' })
    await view.rerender(
      <Harness conversations={[conversation('chat-a'), conversation('chat-b', true)]} />
    )
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a,chat-b')
    await view.rerender(
      <Harness conversations={[conversation('chat-a'), conversation('chat-b')]} />
    )
    await advance()
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
  })

  it('starts and stops the actual conversation immediately on lifecycle events without lighting its peers', async () => {
    const view = await render(<Harness />)
    await advance()
    emit(event(1, { agentId: 'chat-a-child', conversationId: 'chat-a-child' }))
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    emitAgent({ type: 'done', runId: 'run', success: true })
    await expect.element(view.getByTestId('running')).toHaveTextContent('idle')
    emit(event(2, { runId: 'run-2' }))
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    emit(
      event(3, {
        kind: 'turn_updated',
        runId: 'run-2',
        transmission: {
          id: 'completion',
          kind: 'completion',
          sourceAgentId: 'chat-a',
          targetAgentId: null
        }
      })
    )
    await expect.element(view.getByTestId('running')).toHaveTextContent('idle')
  })

  it('does not stop a new turn when an older run reports completion late', async () => {
    const view = await render(<Harness />)
    await advance()
    emit(event(1, { runId: 'older-run' }))
    emit(event(2, { runId: 'newer-run' }))
    emitAgent({ type: 'done', runId: 'older-run', success: true })
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    emitAgent({ type: 'done', runId: 'newer-run', success: true })
    await expect.element(view.getByTestId('running')).toHaveTextContent('idle')
  })

  it('ignores token streams and foreign roots, coalesces progress, and prioritizes lifecycle refreshes', async () => {
    await render(<Harness />)
    await advance()
    mocks.getTree.mockClear()
    for (let index = 0; index < 200; index++)
      emitAgent({ type: 'message_delta', runId: 'run', delta: 'x' })
    emit(event(1, { rootConversationId: 'foreign' }))
    await advance()
    expect(mocks.getTree).not.toHaveBeenCalled()
    for (let sequence = 1; sequence < 10; sequence++)
      emit(event(sequence, { kind: 'turn_updated' }))
    await advance()
    expect(mocks.getTree).not.toHaveBeenCalled()
    mocks.getTree.mockResolvedValue(tree('chat-a', [], 10))
    emit(event(10))
    await advance()
    expect(mocks.getTree).toHaveBeenCalledExactlyOnceWith({ rootConversationId: 'chat-a' })
  })

  it('does not let a stale read overwrite newer events and isolates previous workflow responses', async () => {
    let resolve!: (value: AgentTreeSnapshot) => void
    mocks.getTree.mockImplementation(({ rootConversationId }) =>
      rootConversationId === 'chat-a'
        ? new Promise<AgentTreeSnapshot>((done) => {
            resolve = done
          })
        : Promise.resolve(tree(rootConversationId))
    )
    const view = await render(<Harness />)
    emit(event(1))
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    await act(async () => {
      resolve(tree('chat-a'))
    })
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    await advance()
    const resolveOld = resolve
    await view.rerender(
      <Harness
        instance={workflow({ id: 'second', bindings: [{ nodeId: 'c', conversationId: 'chat-c' }] })}
      />
    )
    await act(async () => {
      resolveOld(tree('chat-a', ['chat-a'], 1))
    })
    await expect.element(view.getByTestId('running')).toHaveTextContent('idle')
  })

  it('retains known activity on failed reads, recovers from resync, and cleans up while unmounted or hidden', async () => {
    mocks.getTree.mockImplementation(async ({ rootConversationId }) =>
      tree(rootConversationId, rootConversationId === 'chat-a' ? ['chat-a'] : [])
    )
    const view = await render(<Harness />)
    await advance()
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    mocks.getTree.mockRejectedValue(new Error('offline'))
    await advance(5_100)
    await expect.element(view.getByTestId('running')).toHaveTextContent('chat-a')
    mocks.getTree.mockImplementation(async ({ rootConversationId }) => tree(rootConversationId))
    mocks.resync.forEach((listener) => listener())
    await advance()
    await expect.element(view.getByTestId('running')).toHaveTextContent('idle')
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden')
    mocks.getTree.mockClear()
    await advance(10_000)
    expect(mocks.getTree).not.toHaveBeenCalled()
    visibility.mockReturnValue('visible')
    document.dispatchEvent(new Event('visibilitychange'))
    await advance()
    expect(mocks.getTree).toHaveBeenCalledTimes(2)
    await view.unmount()
    mocks.getTree.mockClear()
    expect(mocks.agents.size).toBe(0)
    expect(mocks.collaboration.size).toBe(0)
    expect(mocks.resync.size).toBe(0)
    await advance(10_000)
    expect(mocks.getTree).not.toHaveBeenCalled()
  })
})
