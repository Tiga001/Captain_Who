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

  it('keeps retry status through reconnect and clears it on first model output', () => {
    const nextAttempt = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 2
    })
    expect(nextAttempt.agentRun?.llmRetry).toEqual(retryingMessage().agentRun?.llmRetry)

    const firstOutput = applyAgentEventToChatMessage(nextAttempt, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-1',
      delta: '已重新连接'
    })
    expect(firstOutput.agentRun?.llmRetry).toBeUndefined()
  })

  it('clears the transient retry at a terminal boundary', () => {
    const completed = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'done',
      runId: 'run-retry',
      success: true,
      status: 'completed',
      content: 'done'
    })
    expect(completed.agentRun?.llmRetry).toBeUndefined()
  })

  it('clears retry status when a reconnected response starts with tool input', () => {
    const toolInput = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'tool_input_progress',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 2,
      toolCallIndex: 0,
      tool: 'run_command',
      receivedBytes: 12
    })
    expect(toolInput.agentRun?.llmRetry).toBeUndefined()
    expect(toolInput.agentRun?.firstResponseAt).toBeTypeOf('number')
  })

  it('clears retry status when apply_patch starts streaming a preview', () => {
    const preview = applyAgentEventToChatMessage(retryingMessage(), {
      type: 'file_change_preview_updated',
      runId: 'run-retry',
      preview: {
        schemaVersion: 1,
        previewId: 'stream-1:2:0',
        streamId: 'stream-1',
        attempt: 2,
        toolCallIndex: 0,
        toolCallId: null,
        transactionId: 'draft-1',
        filePath: 'src/main.ts',
        additions: 1,
        deletions: 0,
        lineCount: 1,
        byteCount: 12,
        generatedBytes: 12,
        contentOffsetBytes: 0,
        contentDelta: 'export {}',
        updatedAt: 10
      }
    })
    expect(preview.agentRun?.llmRetry).toBeUndefined()
    expect(preview.agentRun?.fileWritePreviews).toHaveLength(1)
  })

  it('binds provisional apply_patch previews to the canonical call and clears them on settlement', () => {
    const preview = applyAgentEventToChatMessage(runningMessage(), {
      type: 'file_change_preview_updated',
      runId: 'run-retry',
      preview: {
        schemaVersion: 1,
        previewId: 'stream-2:1:0',
        streamId: 'stream-2',
        attempt: 1,
        toolCallIndex: 0,
        toolCallId: null,
        transactionId: 'stream-2:1:0',
        filePath: 'src/large.ts',
        additions: 2,
        deletions: 0,
        lineCount: 2,
        byteCount: 18,
        generatedBytes: 18,
        contentOffsetBytes: 0,
        contentDelta: 'private body\nline\n',
        updatedAt: 10
      }
    })
    const called = applyAgentEventToChatMessage(preview, {
      type: 'tool_call',
      runId: 'run-retry',
      traceSequence: 1,
      identity: { type: 'builtin', toolName: 'apply_patch' },
      call: {
        id: 'call-apply-1',
        tool: 'apply_patch',
        args: {
          action: 'apply',
          operation: 'create',
          filePath: 'src/large.ts',
          contentBytes: 18,
          contentDigest: 'file-change-sha256-v1:redacted'
        },
        approvalStatus: 'required',
        reason: null
      }
    })
    expect(called.agentRun?.fileWritePreviews?.[0]?.toolCallId).toBe('call-apply-1')

    const settled = applyAgentEventToChatMessage(called, {
      type: 'tool_result',
      runId: 'run-retry',
      result: {
        callId: 'call-apply-1',
        tool: 'apply_patch',
        ok: false,
        error: 'invalid arguments'
      }
    })
    expect(settled.agentRun?.fileWritePreviews).toEqual([])
  })

  it.each(['already_applied', 'outcome_unknown'] as const)(
    'projects the current FileChange %s terminal status',
    (status) => {
      const next = applyAgentEventToChatMessage(runningMessage(), {
        type: 'file_change_updated',
        runId: 'run-retry',
        fileChange: {
          schemaVersion: 1,
          transactionId: `file-change-${status}`,
          conversationId: 'conversation-1',
          projectId: null,
          filePath: 'src/main.ts',
          operation: 'update',
          updateStrategy: 'modify',
          status,
          baseRevision: 'content-sha256-v1:base',
          additions: 1,
          deletions: 1,
          lineCount: 1,
          byteCount: 4,
          mutationCount: 1,
          nextMutationIndex: 1,
          statsFinal: true,
          summary: null,
          createdAt: 10,
          updatedAt: 11
        }
      })
      expect(next.agentRun?.fileDrafts).toContainEqual(
        expect.objectContaining({ status, transactionId: `file-change-${status}` })
      )
    }
  )

  it('rolls back the failed attempt before projecting the reconnected output', () => {
    const firstAttempt = applyAgentEventToChatMessage(runningMessage(), {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 1
    })
    const staleOutput = applyAgentEventToChatMessage(firstAttempt, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-1',
      delta: '旧的半截回复'
    })
    const reset = applyAgentEventToChatMessage(staleOutput, {
      type: 'message_stream_reset',
      runId: 'run-retry',
      streamId: 'stream-1',
      reason: 'retrying_model_request'
    })
    const reconnecting = applyAgentEventToChatMessage(reset, {
      type: 'llm_retry',
      runId: 'run-retry',
      streamId: 'stream-1',
      category: 'network',
      delayMs: 350,
      retryAt: 1_350,
      attempt: 2,
      maxAttempts: 3
    })
    const secondAttempt = applyAgentEventToChatMessage(reconnecting, {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 2
    })

    expect(secondAttempt.content).toBe('')
    expect(secondAttempt.agentRun?.llmRetry).toBeDefined()

    const recovered = applyAgentEventToChatMessage(secondAttempt, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-1',
      delta: '新的完整回复'
    })
    expect(recovered.content).toBe('新的完整回复')
    expect(recovered.agentRun?.llmRetry).toBeUndefined()
  })

  it('keeps earlier model turns in the timeline without appending them to the final answer', () => {
    const firstStarted = applyAgentEventToChatMessage(runningMessage(), {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-1',
      attempt: 1
    })
    const firstTurn = applyAgentEventToChatMessage(firstStarted, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-1',
      delta: '先读取现有文件。'
    })
    const firstCommitted = applyAgentEventToChatMessage(firstTurn, {
      type: 'message_stream_committed',
      runId: 'run-retry',
      streamId: 'stream-1',
      traceSequence: 1
    })
    const secondStarted = applyAgentEventToChatMessage(firstCommitted, {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-2',
      attempt: 1
    })
    const secondTurn = applyAgentEventToChatMessage(secondStarted, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-2',
      delta: '现在开始实质性升级。'
    })
    const secondCommitted = applyAgentEventToChatMessage(secondTurn, {
      type: 'message_stream_committed',
      runId: 'run-retry',
      streamId: 'stream-2',
      traceSequence: 2
    })
    const finalStarted = applyAgentEventToChatMessage(secondCommitted, {
      type: 'message_stream_started',
      runId: 'run-retry',
      streamId: 'stream-3',
      attempt: 1
    })
    const finalTurn = applyAgentEventToChatMessage(finalStarted, {
      type: 'message_delta',
      runId: 'run-retry',
      streamId: 'stream-3',
      delta: '升级完成。'
    })

    expect(finalTurn.content).toBe('升级完成。')
    expect(
      finalTurn.agentRun?.timeline
        .filter((item) => item.type === 'message')
        .map((item) => item.content)
    ).toEqual(['先读取现有文件。', '现在开始实质性升级。', '升级完成。'])

    const completed = applyAgentEventToChatMessage(finalTurn, {
      type: 'done',
      runId: 'run-retry',
      success: true,
      status: 'completed',
      content: '升级完成。'
    })
    expect(completed.content).toBe('升级完成。')
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
