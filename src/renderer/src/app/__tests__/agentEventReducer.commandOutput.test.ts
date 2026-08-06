import type { AgentEvent } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../../features/agentRun/agentEventReducer'

function message(): ChatMessage {
  const agentRun: ChatAgentRunView = {
    runId: 'run-command-output',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: []
  }
  return {
    id: 'assistant-command-output',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun
  }
}

function outputEvent(
  sequence: number,
  stream: 'stdout' | 'stderr',
  output: string
): Extract<AgentEvent, { type: 'command_output' }> {
  return {
    type: 'command_output',
    runId: 'run-command-output',
    conversationId: 'conversation-command-output',
    assistantMessageId: 'assistant-command-output',
    callId: 'command-call',
    sessionId: 'cmd_1234567890abcdef1234567890abcdef',
    sequence,
    stream,
    output
  }
}

describe('command output runtime projection', () => {
  it('orders and deduplicates chunks by call id, then yields to the final ToolResult', () => {
    const withSecondChunk = applyAgentEventToChatMessage(
      message(),
      outputEvent(2, 'stderr', 'second\n')
    )
    const withBothChunks = applyAgentEventToChatMessage(
      withSecondChunk,
      outputEvent(1, 'stdout', 'first\n')
    )
    const replayed = applyAgentEventToChatMessage(
      withBothChunks,
      outputEvent(1, 'stdout', 'first\n')
    )

    expect(replayed.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toEqual([
      { sequence: 1, stream: 'stdout', output: 'first\n' },
      { sequence: 2, stream: 'stderr', output: 'second\n' }
    ])

    const settled = applyAgentEventToChatMessage(replayed, {
      type: 'tool_result',
      runId: 'run-command-output',
      result: {
        callId: 'command-call',
        tool: 'run_command',
        ok: true,
        result: {
          command: 'printf first',
          stdout: 'first\n',
          stderr: '',
          exitCode: 0
        }
      }
    })

    expect(settled.agentRun?.commandOutputPreviews?.['command-call']).toBeUndefined()
    expect(settled.agentRun?.toolResults).toHaveLength(1)
  })

  it('does not mix output from another run', () => {
    const foreignEvent = {
      ...outputEvent(1, 'stdout', 'foreign'),
      runId: 'another-run'
    }
    expect(applyAgentEventToChatMessage(message(), foreignEvent)).toEqual(message())
  })

  it('updates only the call preview after the assistant message and run are complete', () => {
    const completed: ChatMessage = {
      ...message(),
      content: 'The command was handed off.',
      status: 'sent',
      agentRun: {
        ...message().agentRun!,
        status: 'completed',
        startedAt: 1,
        completedAt: 42,
        fileDrafts: [],
        fileWritePreviews: [],
        messageStreamCheckpoints: {},
        readActivities: []
      }
    }

    const next = applyAgentEventToChatMessage(
      completed,
      outputEvent(1, 'stdout', 'background output\n')
    )

    expect(next).toEqual({
      ...completed,
      agentRun: {
        ...completed.agentRun!,
        commandOutputPreviews: {
          'command-call': {
            callId: 'command-call',
            chunks: [{ sequence: 1, stream: 'stdout', output: 'background output\n' }]
          }
        }
      }
    })
  })
})
