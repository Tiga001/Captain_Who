import { describe, expect, it } from 'vitest'
import type { AgentEvent, AgentObserverEventEnvelope } from '@mycopilot/protocol'
import { applyAgentEventToChatMessage } from '../agentRun/agentEventReducer'
import type { ChatConversation, ChatMessage } from '../chat/chatTypes'
import { applyObserverLiveEnvelope, type ObserverScope } from './observerLiveProjection'

const runId = 'run-parity'
const conversationId = 'conversation-child'
const assistantMessageId = 'assistant-child'

function assistantMessage(): ChatMessage {
  return {
    id: assistantMessageId,
    role: 'assistant',
    content: '',
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
}

function conversation(message: ChatMessage): ChatConversation {
  return {
    id: conversationId,
    projectId: 'project-1',
    modelId: 'model-1',
    title: 'Child',
    messages: [message],
    messagesLoaded: true,
    createdAt: 1,
    updatedAt: 1,
    pinnedAt: null,
    archivedAt: null,
    unreadAt: null,
    continuationOrigin: null
  }
}

const observerScope: ObserverScope = {
  agentId: 'agent-child',
  rootAgentId: 'agent-root',
  rootConversationId: 'conversation-root',
  conversationId
}

function envelope(event: AgentEvent): AgentObserverEventEnvelope {
  return {
    schemaVersion: 1,
    rootAgentId: observerScope.rootAgentId,
    rootConversationId: observerScope.rootConversationId,
    agentId: observerScope.agentId,
    conversationId,
    runId,
    assistantMessageId,
    event
  }
}

const events = [
  { type: 'final_answer_ready', runId },
  {
    type: 'model_activity_changed',
    runId,
    streamId: 'stream-parity',
    attempt: 1,
    activity: 'waiting'
  },
  {
    type: 'tool_set_changed',
    runId,
    stableRevision: 'stable-v1',
    dynamicRevision: 'dynamic-v1',
    effectiveRevision: 'effective-v1',
    toolDefinitions: []
  },
  {
    type: 'todo_updated',
    runId,
    todo: {
      revision: 1,
      items: [
        {
          id: 'todo-1',
          title: 'Verify projection parity',
          status: 'in_progress',
          createdAt: 10,
          updatedAt: 11
        }
      ],
      updatedAt: 11
    }
  },
  {
    type: 'approval_required',
    runId,
    action: {
      type: 'file_change',
      fileChange: {
        schemaVersion: 1,
        id: 'file-change-1',
        transactionId: 'file-change-transaction-1',
        operation: 'update',
        updateStrategy: null,
        filePath: 'README.md',
        inlineDiff: { patch: '@@ -1 +1 @@\n-old\n+new', truncated: false },
        baseRevision: 'sha256-base',
        summary: 'Update the heading',
        additions: 1,
        deletions: 1,
        lineCount: 1,
        byteCount: 4,
        approvalStatus: 'required'
      }
    }
  }
] satisfies AgentEvent[]

describe('observer live projection parity', () => {
  it.each(events.map((event) => [event.type, event] as const))(
    'reduces %s identically in root and child presentation modes',
    (_type, event) => {
      const initial = assistantMessage()
      const direct = applyAgentEventToChatMessage(initial, event)
      const observed = applyObserverLiveEnvelope(
        conversation(initial),
        observerScope,
        envelope(event)
      )

      expect(observed.messages[0]).toEqual(direct)
    }
  )

  it('requires the runtime final marker in both routes and clears it only at settlement', () => {
    const finalEvents: AgentEvent[] = [
      { type: 'message_stream_started', runId, streamId: 'final-stream', attempt: 1 },
      { type: 'message_delta', runId, streamId: 'final-stream', delta: 'The final answer.' },
      { type: 'message_stream_committed', runId, streamId: 'final-stream', traceSequence: null },
      { type: 'final_answer_ready', runId },
      {
        type: 'state',
        runId,
        state: { status: 'running', activeRunId: runId, lastError: null, updatedAt: 1 }
      },
      { type: 'done', runId, success: true, status: 'completed', proposedActions: [] }
    ]
    let direct = assistantMessage()
    let observed = conversation(assistantMessage())
    for (const [index, event] of finalEvents.entries()) {
      direct = applyAgentEventToChatMessage(direct, event)
      observed = applyObserverLiveEnvelope(observed, observerScope, envelope(event))
      expect(observed.messages[0]?.agentRun?.finalAnswerReady).toEqual(
        direct.agentRun?.finalAnswerReady
      )
      expect(Boolean(direct.agentRun?.finalAnswerReady)).toBe(index === 3 || index === 4)
      expect(observed.messages[0]?.agentRun?.status).toBe(index === 5 ? 'completed' : 'running')
    }
  })

  it('projects reasoning, visible output, retry, and terminal boundaries identically', () => {
    const transitionEvents: AgentEvent[] = [
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 1,
        activity: 'waiting'
      },
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 1,
        activity: 'reasoning'
      },
      { type: 'message_stream_started', runId, streamId: 'stream-parity', attempt: 1 },
      { type: 'message_delta', runId, streamId: 'stream-parity', delta: 'visible' },
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 1,
        activity: 'reasoning'
      },
      {
        type: 'message_stream_reset',
        runId,
        streamId: 'stream-parity',
        reason: 'retrying_model_request'
      },
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 2,
        activity: 'waiting'
      },
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 2,
        activity: 'reasoning'
      },
      { type: 'done', runId, success: false, status: 'cancelled', proposedActions: [] },
      {
        type: 'model_activity_changed',
        runId,
        streamId: 'stream-parity',
        attempt: 2,
        activity: 'reasoning'
      }
    ]
    let direct = assistantMessage()
    let observed = conversation(assistantMessage())
    for (const event of transitionEvents) {
      direct = applyAgentEventToChatMessage(direct, event)
      observed = applyObserverLiveEnvelope(observed, observerScope, envelope(event))
      expect(observed.messages[0]?.agentRun?.modelActivity).toEqual(direct.agentRun?.modelActivity)
      expect(observed.messages[0]?.agentRun?.status).toEqual(direct.agentRun?.status)
      expect(observed.messages[0]?.content).toEqual(direct.content)
    }
  })
})
