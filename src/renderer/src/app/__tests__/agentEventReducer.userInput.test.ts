import { describe, expect, it, vi } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import { getActiveRunModelId, getEditableLastTurn } from '../appShellConversationUtils'
import {
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage,
  ensureAgentRun,
  shouldTouchConversationForAgentEvent
} from '../../features/agentRun/agentEventReducer'
import {
  isAssistantMessageGenerating,
  isAssistantReplySettled
} from '../../features/chat/assistantGeneration'
import type { ChatConversation, ChatMessage } from '../../features/chat/chatTypes'
import {
  parsePersistedAgentRun,
  parsePersistedAgentRunJson,
  stringifyPersistedAgentRun
} from '../../features/storage/persistedAgentRun'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

function waitingMessage(): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      ...ensureAgentRun(undefined, 'run-1', 'running'),
      toolCalls: [
        {
          id: 'question-call',
          tool: 'request_user_input',
          args: { questions: [{ title: 'Which branch?' }] },
          approvalStatus: 'not_required',
          reason: null
        }
      ],
      timeline: [{ id: 'question-call', type: 'tool_call', callId: 'question-call' }]
    }
  }
}

const suspended: Extract<AgentEvent, { type: 'done' }> = {
  type: 'done',
  runId: 'run-1',
  success: true,
  status: 'waiting_for_user_input',
  usage: { inputTokens: 10, outputTokens: 2, totalTokens: 12, billableRequestCount: 1 }
}

describe('synchronous human input Run projection', () => {
  it('retains pending occupancy and cumulative usage across suspension, replay and resume', () => {
    const state: AgentEvent = {
      type: 'state',
      runId: 'run-1',
      state: {
        status: 'waiting_for_user_input',
        activeRunId: 'run-1',
        lastError: null,
        updatedAt: 2
      }
    }
    const waiting = applyAgentEventToChatMessage(waitingMessage(), state)
    expect(waiting.status).toBe('pending')
    expect(shouldTouchConversationForAgentEvent(state)).toBe(true)
    const completedSegment = applyAgentEventToChatMessage(waiting, suspended)
    const replayed = applyAgentEventToChatMessage(completedSegment, suspended)
    expect(replayed.agentRun).toMatchObject({
      runId: 'run-1',
      status: 'waiting_for_user_input',
      toolResults: [],
      approvals: [],
      usage: suspended.usage
    })
    expect(replayed.status).toBe('pending')
    expect(replayed.agentRun?.completedAt).toBeUndefined()
    expect(replayed.agentRun?.timeline).toHaveLength(1)

    const restored = parsePersistedAgentRunJson(stringifyPersistedAgentRun(replayed.agentRun))
    expect(restored?.status).toBe('waiting_for_user_input')
    expect(restored?.completedAt).toBeUndefined()
    expect(parsePersistedAgentRun({ ...restored, completedAt: 5 })).toBeUndefined()
    const resumed = applyAgentEventToChatMessage(
      { ...replayed, agentRun: restored },
      { type: 'started', runId: 'run-1', toolDefinitions: [] }
    )
    const answered = applyAgentEventToChatMessage(resumed, {
      type: 'tool_result',
      runId: 'run-1',
      result: {
        callId: 'question-call',
        tool: 'request_user_input',
        ok: true,
        result: { answers: [{ kind: 'text', text: 'main' }] }
      }
    })
    const completed = applyAgentEventToChatMessage(answered, {
      type: 'done',
      runId: 'run-1',
      success: true,
      status: 'completed',
      content: 'Using main.',
      usage: { inputTokens: 24, outputTokens: 5, totalTokens: 29, billableRequestCount: 2 }
    })
    expect(completed.status).toBe('sent')
    expect(completed.agentRun?.runId).toBe('run-1')
    expect(completed.agentRun?.toolCalls).toHaveLength(1)
    expect(completed.agentRun?.toolResults).toHaveLength(1)
    expect(completed.agentRun?.usage?.totalTokens).toBe(29)
    expect(completed.agentRun?.usage?.billableRequestCount).toBe(2)
    expect(completed.agentRun?.completedAt).toBeTypeOf('number')
  })

  it('keeps Stop and active-model guards on a hydrated waiting Run and rejects edit/fork settlement', () => {
    const assistant = applyAgentEventToChatMessage(waitingMessage(), suspended)
    // The durable Run status still owns occupancy if a separately read message status lags.
    assistant.status = 'sent'
    const conversation: ChatConversation = {
      id: 'conversation-1',
      projectId: null,
      modelId: 'model-1',
      title: 'Branch task',
      createdAt: 1,
      updatedAt: 2,
      messages: [
        { id: 'user-1', role: 'user', content: 'Inspect a branch', createdAt: 1, status: 'sent' },
        assistant
      ]
    }
    expect(isAssistantMessageGenerating(assistant)).toBe(true)
    expect(getActiveRunModelId(conversation)).toBe('model-1')
    expect(getEditableLastTurn(conversation)).toBeNull()
    expect(isAssistantReplySettled(assistant)).toBe(false)
  })

  it('cannot resurrect a cancelled Run with a late wait or resume event', () => {
    const waiting = applyAgentEventToChatMessage(waitingMessage(), suspended)
    const cancelled = applyAgentEventToChatMessage(waiting, {
      type: 'done',
      runId: 'run-1',
      success: false,
      status: 'cancelled'
    })
    expect(applyAgentEventToChatMessage(cancelled, suspended)).toBe(cancelled)
    expect(
      applyAgentEventToChatMessage(cancelled, {
        type: 'started',
        runId: 'run-1',
        toolDefinitions: []
      })
    ).toBe(cancelled)
    expect(isAssistantMessageGenerating(cancelled)).toBe(false)
  })

  it('does not let a late approval response resume a human-input pause or regress usage', () => {
    const waiting = applyAgentEventToChatMessage(waitingMessage(), suspended)
    const afterApprovalResponse = applyAgentActionExecutionToChatMessage(waiting, {
      actionId: 'earlier-action',
      actionType: 'tool_call',
      toolName: 'apply_patch',
      status: 'applied',
      agentOutput: {
        content: '',
        status: 'running',
        runId: 'run-1',
        events: [],
        toolDefinitions: [],
        proposedActions: [],
        usage: { totalTokens: 5, billableRequestCount: 1 }
      }
    })
    expect(afterApprovalResponse.status).toBe('pending')
    expect(afterApprovalResponse.agentRun?.status).toBe('waiting_for_user_input')
    expect(afterApprovalResponse.agentRun?.usage).toEqual(suspended.usage)
    expect(afterApprovalResponse.agentRun?.completedAt).toBeUndefined()
  })
})
