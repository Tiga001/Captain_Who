import { describe, expect, it } from 'vitest'
import type {
  AgentEvent,
  AgentObserverEventEnvelope,
  AgentObserverLiveStreamSnapshot
} from '@mycopilot/protocol'
import type { ChatConversation } from '../chat/chatTypes'
import {
  applyObserverLiveEnvelope,
  applyObserverStreamSnapshot,
  applyObserverEnvelopeWithCursor,
  type ObserverScope
} from './observerLiveProjection'

function conversation(id: string, runId: string, messageId: string): ChatConversation {
  return {
    id,
    projectId: 'project-1',
    modelId: 'model-1',
    title: id,
    messages: [
      {
        id: messageId,
        role: 'assistant',
        content: id,
        createdAt: 1,
        status: 'pending',
        attachments: [],
        agentRun: {
          runId,
          status: 'running',
          startedAt: 1,
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          webSearchActivities: [],
          readActivities: [],
          approvals: [],
          fileChangeProposals: [],
          fileChanges: [],
          mcpInvocations: [],
          messageStreamCheckpoints: {},
          timeline: []
        }
      }
    ],
    messagesLoaded: true,
    createdAt: 1,
    updatedAt: 1,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null,
    continuationOrigin: null
  }
}

function scope(agent: string, conversationId: string): ObserverScope {
  return {
    agentId: agent,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    conversationId
  }
}

function envelope(agent: string, conversationId: string, messageId: string, delta: string) {
  return {
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: agent,
    conversationId,
    runId: 'same-run-id',
    assistantMessageId: messageId,
    event: { type: 'message_delta', runId: 'same-run-id', delta }
  } satisfies AgentObserverEventEnvelope
}

describe('observer live projection', () => {
  const liveSnapshot = (): AgentObserverLiveStreamSnapshot => ({
    runId: 'same-run-id',
    assistantMessageId: 'assistant-a',
    cursor: { generation: 'generation-a', sequence: 5 },
    stream: {
      streamId: 'stream-1',
      attempt: 1,
      content: 'complete prefix',
      traceBoundarySequence: 2,
      committed: false
    }
  })

  it('hydrates a final answer marker without text and retains it through older durable replay', () => {
    const snapshot = { ...liveSnapshot(), stream: null, finalAnswerReady: true }
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    initial.messages[0]!.agentRun!.llmRetry = {
      category: 'network',
      delayMs: 1,
      retryAt: 1,
      attempt: 2,
      maxAttempts: 3
    }
    let projected = {
      conversation: applyObserverStreamSnapshot(initial, snapshot),
      position: snapshot as import('./observerLiveProjection').ObserverStreamPosition | null
    }
    expect(projected.conversation.messages[0]?.content).toBe(initial.messages[0]?.content)
    expect(projected.conversation.messages[0]?.agentRun?.status).toBe('running')
    expect(projected.conversation.messages[0]?.agentRun?.llmRetry).toBeUndefined()
    const olderEvents: AgentEvent[] = [
      { type: 'started', runId: 'same-run-id', toolDefinitions: [] },
      {
        type: 'tool_call',
        runId: 'same-run-id',
        traceSequence: 0,
        call: {
          id: 'old-tool',
          tool: 'read_file',
          args: {},
          approvalStatus: 'not_required',
          reason: null
        },
        identity: { type: 'builtin', toolName: 'read_file' }
      },
      {
        type: 'state',
        runId: 'same-run-id',
        state: {
          status: 'waiting_for_approval',
          activeRunId: 'same-run-id',
          lastError: null,
          updatedAt: 1
        }
      }
    ]
    for (const [index, event] of olderEvents.entries()) {
      projected = applyObserverEnvelopeWithCursor(
        projected.conversation,
        scope('agent-a', 'child-a'),
        {
          ...envelope('agent-a', 'child-a', 'assistant-a', ''),
          streamCursor: { ...snapshot.cursor, sequence: index + 1 },
          event
        },
        projected.position
      )
      expect(projected.conversation.messages[0]?.agentRun?.finalAnswerReady).toBe(true)
      expect(projected.conversation.messages[0]?.agentRun?.modelActivity).toBeUndefined()
      expect(projected.conversation.messages[0]?.agentRun?.status).toBe('running')
    }
    expect(projected.conversation.messages[0]?.agentRun?.toolCalls).toHaveLength(1)
  })

  it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
    'hydrates a newer final phase over %s storage without marking it completed',
    (status) => {
      const initial = conversation('child-a', 'same-run-id', 'assistant-a')
      initial.messages[0]!.agentRun!.status = status
      const hydrated = applyObserverStreamSnapshot(initial, {
        ...liveSnapshot(),
        stream: null,
        finalAnswerReady: true
      })
      expect(hydrated.messages[0]?.agentRun?.status).toBe('running')
      expect(hydrated.messages[0]?.status).toBe('pending')
      expect(hydrated.messages[0]?.agentRun?.finalAnswerReady).toBe(true)
    }
  )

  it('rejects an old final marker after a newer request and does not infer finality from a commit', () => {
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    const committed = applyObserverStreamSnapshot(initial, {
      ...liveSnapshot(),
      stream: { ...liveSnapshot().stream!, committed: true }
    })
    expect(committed.messages[0]?.agentRun?.finalAnswerReady).toBeUndefined()
    const snapshot = { ...liveSnapshot(), stream: null, finalAnswerReady: true }
    const resumed = applyObserverEnvelopeWithCursor(
      applyObserverStreamSnapshot(initial, snapshot),
      scope('agent-a', 'child-a'),
      {
        ...envelope('agent-a', 'child-a', 'assistant-a', ''),
        streamCursor: { ...snapshot.cursor, sequence: 6 },
        event: {
          type: 'model_activity_changed',
          runId: 'same-run-id',
          streamId: 'new-request',
          attempt: 1,
          activity: 'waiting'
        }
      },
      snapshot
    )
    expect(resumed.conversation.messages[0]?.agentRun?.finalAnswerReady).toBeUndefined()
    const stale = applyObserverEnvelopeWithCursor(
      resumed.conversation,
      scope('agent-a', 'child-a'),
      {
        ...envelope('agent-a', 'child-a', 'assistant-a', ''),
        streamCursor: snapshot.cursor,
        event: { type: 'final_answer_ready', runId: 'same-run-id' }
      },
      resumed.position
    )
    expect(stale.conversation).toBe(resumed.conversation)
  })

  it.each<AgentEvent>([
    {
      type: 'state',
      runId: 'same-run-id',
      state: {
        status: 'waiting_for_user_input',
        activeRunId: 'same-run-id',
        lastError: null,
        updatedAt: 1
      }
    },
    {
      type: 'error',
      runId: 'same-run-id',
      traceSequence: null,
      message: 'request failed',
      recoverable: true
    },
    { type: 'done', runId: 'same-run-id', success: true, status: 'completed', proposedActions: [] }
  ])('clears hydrated finality for a new $type boundary', (event) => {
    const snapshot = { ...liveSnapshot(), stream: null, finalAnswerReady: true }
    const hydrated = applyObserverStreamSnapshot(
      conversation('child-a', 'same-run-id', 'assistant-a'),
      snapshot
    )
    const next = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...envelope('agent-a', 'child-a', 'assistant-a', ''),
        streamCursor: { ...snapshot.cursor, sequence: 6 },
        event
      },
      snapshot
    )
    expect(next.conversation.messages[0]?.agentRun?.finalAnswerReady).toBeUndefined()
  })

  it('never reactivates a terminal run from a final marker snapshot or event', () => {
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    initial.messages[0]!.agentRun!.status = 'completed'
    const snapshot = { ...liveSnapshot(), stream: null, finalAnswerReady: true }
    expect(applyObserverStreamSnapshot(initial, snapshot).messages[0]).toBe(initial.messages[0])
    expect(
      applyObserverLiveEnvelope(initial, scope('agent-a', 'child-a'), {
        ...envelope('agent-a', 'child-a', 'assistant-a', ''),
        event: { type: 'final_answer_ready', runId: 'same-run-id' }
      })
    ).toBe(initial)
  })

  it('hydrates the full in-flight prefix and only appends unseen text after the snapshot cursor', () => {
    const snapshot = liveSnapshot()
    const hydrated = applyObserverStreamSnapshot(
      conversation('child-a', 'same-run-id', 'assistant-a'),
      snapshot
    )
    const event = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ''),
      streamCursor: snapshot.cursor,
      event: { type: 'message_delta', runId: 'same-run-id', streamId: 'stream-1', delta: 'prefix' }
    } satisfies AgentObserverEventEnvelope
    const duplicate = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      event,
      snapshot
    )
    expect(duplicate.conversation).toBe(hydrated)
    const next = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...event,
        streamCursor: { ...snapshot.cursor, sequence: 6 },
        event: { ...event.event, delta: ' + suffix' }
      },
      snapshot
    )
    expect(next.conversation.messages[0]?.content).toBe('complete prefix + suffix')
    expect(next.conversation.messages[0]?.agentRun?.timeline).toMatchObject([
      { type: 'message', content: 'complete prefix + suffix' }
    ])
  })

  it('hydrates reasoning without public text and ignores older activity or durable started replay', () => {
    const modelActivity = { streamId: 'stream-private', attempt: 2, activity: 'reasoning' as const }
    const snapshot = { ...liveSnapshot(), stream: null, modelActivity }
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    const hydrated = applyObserverStreamSnapshot(initial, snapshot)
    expect(hydrated.messages[0]?.agentRun?.modelActivity).toEqual(modelActivity)
    expect(hydrated.messages[0]?.content).toBe(initial.messages[0]?.content)
    expect(hydrated.messages[0]?.agentRun?.timeline).toEqual([])
    expect(hydrated.messages[0]?.agentRun?.modelActivityAttempts).toEqual({ 'stream-private': 2 })

    const stale = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ''),
      streamCursor: { ...snapshot.cursor, sequence: 4 },
      event: {
        type: 'model_activity_changed',
        runId: 'same-run-id',
        ...modelActivity,
        activity: 'waiting'
      }
    } satisfies AgentObserverEventEnvelope
    expect(
      applyObserverEnvelopeWithCursor(hydrated, scope('agent-a', 'child-a'), stale, snapshot)
        .conversation
    ).toBe(hydrated)
    const replay = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...stale,
        event: { type: 'started', runId: 'same-run-id', toolDefinitions: [] }
      },
      snapshot
    )
    expect(replay.conversation.messages[0]?.agentRun?.modelActivity).toEqual(modelActivity)
    expect(replay.position).toBe(snapshot)
  })

  it('preserves newer reasoning while replaying a durable tool event from before the snapshot', () => {
    const snapshot = {
      ...liveSnapshot(),
      stream: null,
      modelActivity: {
        streamId: 'stream-private',
        attempt: 1,
        activity: 'reasoning' as const
      }
    }
    const hydrated = applyObserverStreamSnapshot(
      conversation('child-a', 'same-run-id', 'assistant-a'),
      snapshot
    )
    const replay = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...envelope('agent-a', 'child-a', 'assistant-a', ''),
        streamCursor: { ...snapshot.cursor, sequence: 4 },
        event: {
          type: 'tool_call',
          runId: 'same-run-id',
          traceSequence: 1,
          call: {
            id: 'tool-1',
            tool: 'read_file',
            args: {},
            approvalStatus: 'not_required',
            reason: null
          },
          identity: { type: 'builtin', toolName: 'read_file' }
        }
      },
      snapshot
    )
    expect(replay.conversation.messages[0]?.agentRun?.modelActivity).toEqual(snapshot.modelActivity)
    expect(replay.conversation.messages[0]?.agentRun?.toolCalls).toHaveLength(1)
  })

  it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
    'resumes a %s durable run when the snapshot proves a newer active request',
    (status) => {
      const initial = conversation('child-a', 'same-run-id', 'assistant-a')
      initial.messages[0]!.agentRun!.status = status
      const snapshot = {
        ...liveSnapshot(),
        stream: null,
        modelActivity: {
          streamId: 'resumed-stream',
          attempt: 1,
          activity: 'reasoning' as const
        }
      }
      const hydrated = applyObserverStreamSnapshot(initial, snapshot)
      expect(hydrated.messages[0]?.agentRun?.status).toBe('running')
      expect(hydrated.messages[0]?.status).toBe('pending')
      const next = applyObserverEnvelopeWithCursor(
        hydrated,
        scope('agent-a', 'child-a'),
        {
          ...envelope('agent-a', 'child-a', 'assistant-a', ''),
          streamCursor: { ...snapshot.cursor, sequence: 6 },
          event: {
            type: 'model_activity_changed',
            runId: 'same-run-id',
            ...snapshot.modelActivity,
            activity: 'waiting'
          }
        },
        snapshot
      )
      expect(next.conversation.messages[0]?.agentRun?.modelActivity?.activity).toBe('waiting')
    }
  )

  it('advances activity ordering for no-op events and prevents late activity after terminal', () => {
    const snapshot = {
      ...liveSnapshot(),
      stream: null,
      modelActivity: {
        streamId: 'stream-private',
        attempt: 1,
        activity: 'reasoning' as const
      }
    }
    const hydrated = applyObserverStreamSnapshot(
      conversation('child-a', 'same-run-id', 'assistant-a'),
      snapshot
    )
    const base = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ''),
      streamCursor: { ...snapshot.cursor, sequence: 6 }
    }
    const noOp = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...base,
        event: { type: 'model_activity_changed', runId: 'same-run-id', ...snapshot.modelActivity }
      },
      snapshot
    )
    expect(noOp.position?.cursor.sequence).toBe(6)
    const ended = applyObserverEnvelopeWithCursor(
      noOp.conversation,
      scope('agent-a', 'child-a'),
      {
        ...base,
        streamCursor: { ...snapshot.cursor, sequence: 7 },
        event: {
          type: 'done',
          runId: 'same-run-id',
          success: false,
          status: 'cancelled',
          proposedActions: []
        }
      },
      noOp.position
    )
    expect(ended.conversation.messages[0]?.agentRun?.modelActivity).toBeUndefined()
    const late = applyObserverEnvelopeWithCursor(
      ended.conversation,
      scope('agent-a', 'child-a'),
      {
        ...base,
        streamCursor: { ...snapshot.cursor, sequence: 8 },
        event: {
          type: 'model_activity_changed',
          runId: 'same-run-id',
          ...snapshot.modelActivity,
          activity: 'waiting'
        }
      },
      ended.position
    )
    expect(late.conversation.messages[0]?.agentRun?.modelActivity).toBeUndefined()
    expect(late.conversation.messages[0]?.agentRun?.status).toBe('cancelled')
  })

  it('keeps a resumed snapshot reasoning while older pause and resume states replay', () => {
    const snapshot = {
      ...liveSnapshot(),
      stream: null,
      modelActivity: {
        streamId: 'resumed-stream',
        attempt: 1,
        activity: 'reasoning' as const
      }
    }
    let projected = {
      conversation: applyObserverStreamSnapshot(
        conversation('child-a', 'same-run-id', 'assistant-a'),
        snapshot
      ),
      position: snapshot as import('./observerLiveProjection').ObserverStreamPosition | null
    }
    for (const [index, status] of (['waiting_for_approval', 'running'] as const).entries()) {
      projected = applyObserverEnvelopeWithCursor(
        projected.conversation,
        scope('agent-a', 'child-a'),
        {
          ...envelope('agent-a', 'child-a', 'assistant-a', ''),
          streamCursor: { ...snapshot.cursor, sequence: index + 1 },
          event: {
            type: 'state',
            runId: 'same-run-id',
            state: {
              status,
              activeRunId: 'same-run-id',
              lastError: null,
              updatedAt: index + 1
            }
          }
        },
        projected.position
      )
      expect(projected.conversation.messages[0]?.agentRun?.modelActivity).toEqual(
        snapshot.modelActivity
      )
      expect(projected.conversation.messages[0]?.agentRun?.status).toBe('running')
    }
  })

  it('keeps prior narration and avoids duplicating a narration committed just before its notification', () => {
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    initial.messages[0]!.agentRun!.timeline = [
      { id: 'trace-message-0', type: 'message', content: 'earlier narration', traceSequence: 0 },
      { id: 'trace-message-2', type: 'message', content: 'complete prefix', traceSequence: 2 },
      { id: 'tool-next', type: 'tool_call', callId: 'next', traceSequence: 3 }
    ]
    const hydrated = applyObserverStreamSnapshot(initial, liveSnapshot())
    expect(hydrated.messages[0]?.agentRun?.timeline).toMatchObject([
      { content: 'earlier narration', traceSequence: 0 },
      { content: 'complete prefix', streamId: 'stream-1' },
      { callId: 'next', traceSequence: 3 }
    ])
  })

  it('lets a retry reset snapshot text and does not filter non-text events by the text cursor', () => {
    const snapshot = liveSnapshot()
    const hydrated = applyObserverStreamSnapshot(
      conversation('child-a', 'same-run-id', 'assistant-a'),
      snapshot
    )
    const base = envelope('agent-a', 'child-a', 'assistant-a', '')
    const afterState = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...base,
        streamCursor: { ...snapshot.cursor, sequence: 4 },
        event: { type: 'started', runId: 'same-run-id', toolDefinitions: [] }
      },
      snapshot
    )
    expect(afterState.conversation).not.toBe(hydrated)
    const reset = applyObserverEnvelopeWithCursor(
      hydrated,
      scope('agent-a', 'child-a'),
      {
        ...base,
        streamCursor: { ...snapshot.cursor, sequence: 6 },
        event: {
          type: 'message_stream_reset',
          runId: 'same-run-id',
          streamId: 'stream-1',
          reason: 'retrying_model_request'
        }
      },
      snapshot
    )
    expect(reset.conversation.messages[0]?.content).toBe('child-a')
    expect(reset.conversation.messages[0]?.agentRun?.timeline).toEqual([])
  })

  it('never replaces a durable terminal answer with provisional snapshot text', () => {
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    initial.messages[0]!.content = 'final answer'
    initial.messages[0]!.agentRun!.status = 'completed'
    expect(applyObserverStreamSnapshot(initial, liveSnapshot()).messages[0]).toBe(
      initial.messages[0]
    )
  })

  it('isolates two simultaneous child streams even if a forged Host reuses a run id', () => {
    const childA = conversation('child-a', 'same-run-id', 'assistant-a')
    const childB = conversation('child-b', 'same-run-id', 'assistant-b')
    const eventA = envelope('agent-a', 'child-a', 'assistant-a', ' +A')
    const eventB = envelope('agent-b', 'child-b', 'assistant-b', ' +B')

    const nextA = applyObserverLiveEnvelope(
      applyObserverLiveEnvelope(childA, scope('agent-a', 'child-a'), eventB),
      scope('agent-a', 'child-a'),
      eventA
    )
    const nextB = applyObserverLiveEnvelope(
      applyObserverLiveEnvelope(childB, scope('agent-b', 'child-b'), eventA),
      scope('agent-b', 'child-b'),
      eventB
    )

    expect(nextA.messages[0]?.content).toBe('child-a +A')
    expect(nextB.messages[0]?.content).toBe('child-b +B')
  })

  it('uses the shared reducer to reset a failed stream attempt before retry content', () => {
    const initial = conversation('child-a', 'same-run-id', 'assistant-a')
    const observerScope = scope('agent-a', 'child-a')
    const started = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ''),
      event: {
        type: 'message_stream_started',
        runId: 'same-run-id',
        streamId: 'stream-1',
        attempt: 1
      }
    } satisfies AgentObserverEventEnvelope
    const delta = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ' obsolete'),
      event: {
        type: 'message_delta',
        runId: 'same-run-id',
        streamId: 'stream-1',
        delta: ' obsolete'
      }
    } satisfies AgentObserverEventEnvelope
    const reset = {
      ...envelope('agent-a', 'child-a', 'assistant-a', ''),
      event: {
        type: 'message_stream_reset',
        runId: 'same-run-id',
        streamId: 'stream-1',
        reason: 'retrying_model_request'
      }
    } satisfies AgentObserverEventEnvelope

    const afterReset = applyObserverLiveEnvelope(
      applyObserverLiveEnvelope(
        applyObserverLiveEnvelope(initial, observerScope, started),
        observerScope,
        delta
      ),
      observerScope,
      reset
    )

    expect(afterReset.messages[0]?.content).toBe('child-a')
    expect(afterReset.messages[0]?.agentRun?.messageStreamCheckpoints).toEqual({})
  })
})
