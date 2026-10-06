import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '@mycopilot/protocol'
import type { ChatMessage } from '../../features/chat/chatTypes'
import {
  applyAgentEventToChatMessage as apply,
  ensureAgentRun,
  shouldTouchConversationForAgentEvent
} from '../../features/agentRun/agentEventReducer'

const ready: Extract<AgentEvent, { type: 'final_answer_ready' }> = {
  type: 'final_answer_ready',
  runId: 'run-final'
}
function message(): ChatMessage {
  return {
    id: 'assistant-final',
    role: 'assistant',
    content: 'Final response.',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      ...ensureAgentRun(undefined, ready.runId, 'running'),
      modelActivity: { streamId: 'stream-final', attempt: 1, activity: 'waiting' },
      modelActivityAttempts: { 'stream-final': 1 },
      toolCalls: [
        {
          id: 'earlier-call',
          tool: 'read_file',
          args: {},
          approvalStatus: 'not_required',
          reason: null
        }
      ],
      toolResults: [
        { callId: 'earlier-call', tool: 'read_file', ok: true, result: 'Earlier result' }
      ]
    }
  }
}

describe('final answer readiness projection', () => {
  it('accepts only explicit matching readiness without completing the run or consulting earlier tools', () => {
    const original = message()
    const committed = apply(original, {
      type: 'message_stream_committed',
      runId: ready.runId,
      streamId: 'stream-final',
      traceSequence: null
    })
    expect(committed.agentRun?.finalAnswerReady).toBeUndefined()
    const final = apply(committed, ready)
    expect(final.agentRun?.finalAnswerReady).toBe(true)
    expect(final.agentRun?.modelActivity).toBeUndefined()
    expect(final.agentRun?.llmRetry).toBeUndefined()
    expect(final.agentRun?.status).toBe('running')
    expect(final.agentRun?.completedAt).toBeUndefined()
    expect(final.status).toBe('pending')
    expect(final.content).toBe(original.content)
    expect(final.agentRun?.toolCalls).toEqual(original.agentRun?.toolCalls)
    expect(apply(final, ready)).toBe(final)
    expect(apply(original, { ...ready, runId: 'wrong-run' })).toBe(original)
    const unbound = { ...original, agentRun: undefined }
    expect(apply(unbound, ready)).toBe(unbound)
    expect(shouldTouchConversationForAgentEvent(ready)).toBe(false)
  })

  it.each([
    { type: 'started', runId: ready.runId, toolDefinitions: [] },
    { type: 'message_stream_started', runId: ready.runId, streamId: 'next-stream', attempt: 1 },
    {
      type: 'model_activity_changed',
      runId: ready.runId,
      streamId: 'next-stream',
      attempt: 1,
      activity: 'waiting'
    },
    { type: 'message_stream_reset', runId: ready.runId, streamId: 'stream-final', reason: 'retry' },
    {
      type: 'llm_retry',
      runId: ready.runId,
      streamId: 'stream-final',
      attempt: 2,
      maxAttempts: 3,
      category: 'network',
      delayMs: 1000,
      retryAt: 2000
    },
    {
      type: 'error',
      runId: ready.runId,
      message: 'Failed to finish',
      recoverable: true,
      traceSequence: null
    },
    {
      type: 'done',
      runId: ready.runId,
      status: 'completed',
      success: true,
      content: 'Final response.'
    }
  ] satisfies AgentEvent[])('clears the marker on $type', (event) => {
    expect(apply(apply(message(), ready), event).agentRun?.finalAnswerReady).toBeUndefined()
  })

  it.each([
    'waiting_for_approval',
    'waiting_for_user_input',
    'failed',
    'cancelled',
    'completed'
  ] as const)('clears and rejects readiness after %s', (status) => {
    const stopped = apply(apply(message(), ready), {
      type: 'state',
      runId: ready.runId,
      state: { status, activeRunId: ready.runId, lastError: null, updatedAt: 2 }
    })
    expect(stopped.agentRun?.finalAnswerReady).toBeUndefined()
    expect(apply(stopped, ready)).toBe(stopped)
  })

  it('rejects old activity without erasing the final marker and clears it on a new request', () => {
    const final = apply(message(), ready)
    const oldActivity: AgentEvent = {
      type: 'model_activity_changed',
      runId: ready.runId,
      streamId: 'stream-final',
      attempt: 1,
      activity: 'reasoning'
    }
    expect(apply(final, oldActivity)).toBe(final)
    const next = apply(final, { ...oldActivity, streamId: 'next-stream', activity: 'waiting' })
    expect(next.agentRun?.finalAnswerReady).toBeUndefined()
    expect(next.agentRun?.modelActivity?.activity).toBe('waiting')
  })
})
