import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

function runningMessage(): ChatMessage {
  return {
    id: 'assistant-safe-interruption',
    role: 'assistant',
    content: '此前已经提交的有效内容。',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-safe-interruption',
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

describe('safe terminal model request interruption', () => {
  it('drops the failed sampling attempt while preserving the committed prefix', () => {
    const started = applyAgentEventToChatMessage(runningMessage(), {
      type: 'message_stream_started',
      runId: 'run-safe-interruption',
      streamId: 'failed-attempt',
      attempt: 1
    })
    const provisional = applyAgentEventToChatMessage(started, {
      type: 'message_delta',
      runId: 'run-safe-interruption',
      streamId: 'failed-attempt',
      delta: '这段未提交内容必须被断尾。'
    })
    const interrupted = applyAgentEventToChatMessage(provisional, {
      type: 'error',
      runId: 'run-safe-interruption',
      traceSequence: null,
      message: 'raw provider transport diagnostic must not become assistant content',
      recoverable: false,
      code: 'agent.model_request_interrupted',
      details: {
        type: 'safe_model_request_interruption',
        reason: 'service_connection_failed',
        safeToContinue: true
      }
    })
    const completed = applyAgentEventToChatMessage(interrupted, {
      type: 'done',
      runId: 'run-safe-interruption',
      success: false,
      status: 'failed',
      content: 'raw terminal content must not be restored'
    })

    expect(completed.content).toBe('此前已经提交的有效内容。')
    expect(completed.status).toBe('sent')
    expect(completed.agentRun?.status).toBe('failed')
    expect(completed.agentRun?.interruption).toEqual({ reason: 'service_connection_failed' })
    expect(completed.agentRun?.error).toBeUndefined()
    expect(completed.agentRun?.timeline.some((item) => item.type === 'error')).toBe(false)
    expect(JSON.stringify(completed)).not.toContain('这段未提交内容必须被断尾')
    expect(JSON.stringify(completed)).not.toContain('raw provider transport diagnostic')
    expect(JSON.stringify(completed)).not.toContain('raw terminal content')
  })

  it('keeps unmarked terminal failures on the existing hard-error path', () => {
    const failed = applyAgentEventToChatMessage(runningMessage(), {
      type: 'error',
      runId: 'run-safe-interruption',
      traceSequence: null,
      message: '工具执行状态无法持久化',
      recoverable: false,
      code: 'agent.storage_failed',
      details: { type: 'storage_failure' }
    })

    expect(failed.content).toBe('工具执行状态无法持久化')
    expect(failed.status).toBe('error')
    expect(failed.agentRun?.status).toBe('failed')
    expect(failed.agentRun?.interruption).toBeUndefined()
    expect(failed.agentRun?.error).toBe('工具执行状态无法持久化')
    expect(failed.agentRun?.timeline.some((item) => item.type === 'error')).toBe(true)
  })
})
