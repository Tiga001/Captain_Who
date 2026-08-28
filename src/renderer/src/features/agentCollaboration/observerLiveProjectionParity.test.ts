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
      diffs: [],
      fileDrafts: [],
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
})
