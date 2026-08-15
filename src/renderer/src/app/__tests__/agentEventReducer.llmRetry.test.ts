import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

function runningMessage(): ChatMessage {
  return {
    id: 'assistant-retry',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-retry',
      status: 'running',
      startedAt: 1,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      diffs: [],
      timeline: [],
      messageStreamCheckpoints: {}
    }
  }
}

function retryingMessage(): ChatMessage {
  return applyAgentEventToChatMessage(runningMessage(), {
    type: 'llm_retry',
    runId: 'run-retry',
    streamId: 'stream-1',
    category: 'rate_limited',
    providerCode: 'rate_limit_exceeded',
    delayMs: 5_000,
    retryAt: 15_000,
    attempt: 2,
    maxAttempts: 3
  })
}

describe('LLM retry Renderer projection', () => {
  it('projects only bounded structured retry metadata', () => {
    expect(retryingMessage().agentRun?.llmRetry).toEqual({
      category: 'rate_limited',
      providerCode: 'rate_limit_exceeded',
      delayMs: 5_000,
      retryAt: 15_000,
      attempt: 2,
      maxAttempts: 3
    })
  })

  it('clears the transient retry at the next model attempt and terminal boundary', () => {
    const nextAttempt = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 2
    })
    expect(nextAttempt.agentRun?.llmRetry).toBeUndefined()

    const completed = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'done',
      runId: 'run-retry',
      success: true,
      status: 'completed',
      content: 'done'
    })
    expect(completed.agentRun?.llmRetry).toBeUndefined()
  })

  it('does not let a late retry resurrect a terminal run', () => {
    const completed = applyAgentEventToChatMessage(runningMessage(), {
      type: 'done',
      runId: 'run-retry',
      success: false,
      status: 'cancelled',
      content: ''
    })
    const lateRetry = applyAgentEventToChatMessage(completed, {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'unknown',
      delayMs: 5_000,
      retryAt: 15_000,
      attempt: 2,
      maxAttempts: 3
    })
    expect(lateRetry).toBe(completed)
  })

  it('keeps cancelled model output in the timeline without treating it as a final answer', () => {
    const streaming = applyAgentEventToChatMessage(runningMessage(), {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-cancelled',
      delta: '这是停止前的过程说明。'
    })
    const cancelled = applyAgentEventToChatMessage(streaming, {
      type: 'done',
      runId: 'run-retry',
      success: false,
      status: 'cancelled',
      content: '这段内容也不能成为最终回复。'
    })

    expect(cancelled.content).toBe('')
    expect(cancelled.agentRun?.status).toBe('cancelled')
    expect(cancelled.agentRun?.timeline).toEqual([
      {
        id: 'message-stream-stream-cancelled',
        type: 'message',
        content: '这是停止前的过程说明。',
        streamId: 'stream-cancelled'
      }
    ])
  })
})
