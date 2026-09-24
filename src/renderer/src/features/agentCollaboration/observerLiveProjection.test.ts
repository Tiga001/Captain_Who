import { describe, expect, it } from 'vitest'
import type {
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
