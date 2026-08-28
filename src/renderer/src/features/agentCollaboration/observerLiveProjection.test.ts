import { describe, expect, it } from 'vitest'
import type { AgentObserverEventEnvelope } from '@mycopilot/protocol'
import type { ChatConversation } from '../chat/chatTypes'
import { applyObserverLiveEnvelope, type ObserverScope } from './observerLiveProjection'

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
