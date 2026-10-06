import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import type { ChatMessage } from '../../features/chat/chatTypes'
import {
  applyAgentEventToChatMessage,
  ensureAgentRun,
  shouldTouchConversationForAgentEvent
} from '../../features/agentRun/agentEventReducer'

function message(): ChatMessage {
  return {
    id: 'assistant-activity',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: ensureAgentRun(undefined, 'run-activity', 'running')
  }
}

const waiting: Extract<AgentEvent, { type: 'model_activity_changed' }> = {
  type: 'model_activity_changed',
  runId: 'run-activity',
  streamId: 'stream-activity',
  attempt: 1,
  activity: 'waiting'
}
const reasoning = { ...waiting, activity: 'reasoning' as const }
const apply = applyAgentEventToChatMessage
const activeMessage = () => apply(apply(message(), waiting), reasoning)

describe('transient model activity projection', () => {
  it('requires positive matching evidence for reasoning, even without a public text stream', () => {
    const initial = message()
    expect(apply(initial, reasoning)).toBe(initial)
    const pending = apply(initial, waiting)
    expect(pending.agentRun?.modelActivity?.activity).toBe('waiting')
    const active = apply(pending, reasoning)
    expect(active.agentRun?.modelActivity).toEqual({
      streamId: waiting.streamId,
      attempt: 1,
      activity: 'reasoning'
    })
    expect(apply(active, reasoning)).toBe(active)
    expect(apply(active, { ...reasoning, runId: 'other-run' })).toBe(active)
    expect(apply(active, { ...reasoning, streamId: 'other-stream' })).toBe(active)
    expect(apply(active, { ...reasoning, attempt: 2 })).toBe(active)
    expect(shouldTouchConversationForAgentEvent(reasoning)).toBe(false)
    expect(active.agentRun?.firstResponseAt).toBeUndefined()
    expect(active.agentRun?.timeline).toEqual([])
  })

  it('returns to waiting on text/tool output and permits later reasoning in the same attempt', () => {
    const active = activeMessage()
    const text = apply(active, {
      type: 'message_delta',
      runId: waiting.runId,
      streamId: waiting.streamId,
      delta: 'Working.'
    })
    expect(text.agentRun?.modelActivity?.activity).toBe('waiting')
    const reasoningAgain = apply(text, reasoning)
    expect(reasoningAgain.agentRun?.modelActivity?.activity).toBe('reasoning')
    const staleToolProgress = apply(reasoningAgain, {
      type: 'tool_input_progress',
      runId: waiting.runId,
      streamId: waiting.streamId,
      attempt: 0,
      toolCallIndex: 0,
      tool: 'read_file',
      receivedBytes: 10
    })
    expect(staleToolProgress.agentRun?.modelActivity?.activity).toBe('reasoning')
    const tool = apply(reasoningAgain, {
      type: 'tool_input_progress',
      runId: waiting.runId,
      streamId: waiting.streamId,
      attempt: 1,
      toolCallIndex: 0,
      tool: 'read_file',
      receivedBytes: 10
    })
    expect(tool.agentRun?.modelActivity?.activity).toBe('waiting')
    expect(
      apply(active, { ...waiting, activity: 'waiting' }).agentRun?.modelActivity?.activity
    ).toBe('waiting')
  })

  it('retires a reset attempt, rejects its late events, and accepts the next retry attempt', () => {
    const active = activeMessage()
    const reset = apply(active, {
      type: 'message_stream_reset',
      runId: waiting.runId,
      streamId: waiting.streamId,
      reason: 'retry'
    })
    expect(reset.agentRun?.modelActivity).toBeUndefined()
    expect(apply(reset, waiting)).toBe(reset)
    expect(apply(reset, reasoning)).toBe(reset)
    const retry = apply(reset, {
      type: 'llm_retry',
      runId: waiting.runId,
      streamId: waiting.streamId,
      category: 'network',
      delayMs: 1000,
      retryAt: 2000,
      attempt: 2,
      maxAttempts: 3
    })
    const next = apply(retry, { ...waiting, attempt: 2 })
    expect(next.agentRun?.modelActivity?.activity).toBe('waiting')
    expect(next.agentRun?.llmRetry).toBeDefined()
    const nextReasoning = apply(next, { ...reasoning, attempt: 2 })
    expect(nextReasoning.agentRun?.modelActivity?.activity).toBe('reasoning')
    expect(nextReasoning.agentRun?.llmRetry).toBeUndefined()
    expect(apply(nextReasoning, waiting)).toBe(nextReasoning)
    expect(apply(nextReasoning, reasoning)).toBe(nextReasoning)
    const laterStream = apply(nextReasoning, { ...waiting, streamId: 'stream-next' })
    expect(apply(laterStream, { ...waiting, attempt: 2 })).toBe(laterStream)
  })

  it('ignores retries for another stream or an attempt that has already started', () => {
    const active = apply(apply(message(), { ...waiting, attempt: 2 }), { ...reasoning, attempt: 2 })
    const retry: Extract<AgentEvent, { type: 'llm_retry' }> = {
      type: 'llm_retry',
      runId: waiting.runId,
      streamId: 'retired-stream',
      category: 'network',
      delayMs: 1000,
      retryAt: 2000,
      attempt: 2,
      maxAttempts: 3
    }
    expect(apply(active, retry)).toBe(active)
    expect(apply(active, { ...retry, streamId: waiting.streamId })).toBe(active)
    expect(apply(active, { ...reasoning, attempt: 2 })).toBe(active)
    const reset = apply(active, {
      type: 'message_stream_reset',
      runId: waiting.runId,
      streamId: waiting.streamId,
      reason: 'retry'
    })
    expect(apply(reset, { ...retry, streamId: waiting.streamId })).toBe(reset)
    const nextRetry = apply(active, { ...retry, streamId: waiting.streamId, attempt: 3 })
    expect(nextRetry.agentRun?.modelActivity).toBeUndefined()
    expect(nextRetry.agentRun?.llmRetry?.attempt).toBe(3)
  })

  it.each([
    'waiting_for_approval',
    'waiting_for_user_input',
    'failed',
    'cancelled',
    'completed'
  ] as const)('clears activity at %s and rejects late reasoning', (status) => {
    const settled = apply(activeMessage(), {
      type: 'state',
      runId: waiting.runId,
      state: { status, activeRunId: waiting.runId, lastError: null, updatedAt: 2 }
    })
    expect(settled.agentRun?.modelActivity).toBeUndefined()
    expect(apply(settled, reasoning)).toBe(settled)
    expect(apply(settled, waiting)).toBe(settled)
  })

  it('clears recoverable errors and final completion without changing the final answer', () => {
    const error = apply(activeMessage(), {
      type: 'error',
      runId: waiting.runId,
      message: 'Temporary failure',
      recoverable: true,
      traceSequence: null
    })
    expect(error.agentRun?.modelActivity).toBeUndefined()
    const done = apply(activeMessage(), {
      type: 'done',
      runId: waiting.runId,
      success: true,
      status: 'completed',
      content: 'Final answer.'
    })
    expect(done.agentRun?.modelActivity).toBeUndefined()
    expect(done.content).toBe('Final answer.')
    expect(done.status).toBe('sent')
  })
})
