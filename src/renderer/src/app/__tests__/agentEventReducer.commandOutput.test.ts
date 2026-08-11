import type { AgentEvent } from '@mycopilot/protocol'
import { AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../../features/chat/chatTypes'
import {
  applyAgentActionExecutionToChatMessage,
  applyAgentCommandSessionSnapshotToChatMessage,
  applyAgentEventToChatMessage,
  markMissingAgentCommandSessionOutcomeUnknown,
  shouldTouchConversationForAgentEvent
} from '../../features/agentRun/agentEventReducer'

function message(): ChatMessage {
  const agentRun: ChatAgentRunView = {
    runId: 'run-command-output',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [
      {
        id: 'command-call',
        tool: 'run_command',
        args: { command: 'printf first' },
        approvalStatus: 'approved'
      }
    ],
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
  it('keeps a managed process authoritative over generic ToolResult settlement', () => {
    const withFirstChunk = applyAgentEventToChatMessage(
      message(),
      outputEvent(1, 'stdout', 'first\n')
    )
    const withBothChunks = applyAgentEventToChatMessage(
      withFirstChunk,
      outputEvent(2, 'stderr', 'second\n')
    )
    const replayed = applyAgentEventToChatMessage(
      withBothChunks,
      outputEvent(1, 'stdout', 'first\n')
    )

    expect(replayed.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toEqual([
      { sequence: 1, stream: 'stdout', output: 'first\n' },
      { sequence: 2, stream: 'stderr', output: 'second\n' }
    ])
    expect(replayed.agentRun?.commandSessions?.['command-call']).toMatchObject({
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      status: 'running',
      latestSequence: 2
    })

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

    expect(settled.agentRun?.commandSessions?.['command-call']?.status).toBe('running')
    expect(settled.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toHaveLength(2)
    expect(settled.agentRun?.toolResults).toHaveLength(1)

    const exited = applyAgentEventToChatMessage(settled, {
      type: 'command_exited',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      status: 'exited',
      exitCode: 0,
      endedAt: 50,
      latestSequence: 2,
      outputTruncated: false,
      outputs: [
        {
          name: 'pages/page-1.png',
          kind: 'image',
          readPath: `image-artifact://sha256/${'a'.repeat(64)}`,
          mimeType: 'image/png',
          sizeBytes: 2048,
          sha256: 'a'.repeat(64),
          width: 1200,
          height: 1600
        }
      ]
    })
    expect(exited.agentRun?.commandSessions?.['command-call']?.status).toBe('exited')
    expect(exited.agentRun?.commandSessions?.['command-call']?.outputs?.[0]?.readPath).toBe(
      `image-artifact://sha256/${'a'.repeat(64)}`
    )
    expect(exited.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toHaveLength(2)
  })

  it('still settles a pre-start command failure without a managed Session identity', () => {
    const failed = applyAgentEventToChatMessage(message(), {
      type: 'tool_result',
      runId: 'run-command-output',
      result: {
        callId: 'command-call',
        tool: 'run_command',
        ok: false,
        error: 'spawn failed'
      }
    })

    expect(failed.agentRun?.commandSessions?.['command-call']).toMatchObject({
      status: 'failed',
      latestSequence: 0
    })
    expect(failed.agentRun?.commandSessions?.['command-call']?.sessionId).toBeUndefined()
  })

  it('does not mix output from another run', () => {
    const foreignEvent = {
      ...outputEvent(1, 'stdout', 'foreign'),
      runId: 'another-run'
    }
    expect(applyAgentEventToChatMessage(message(), foreignEvent)).toEqual(message())
  })

  it('ignores an older sequence while the process is still running', () => {
    const second = applyAgentEventToChatMessage(message(), outputEvent(2, 'stdout', 'second\n'))
    const staleFirst = applyAgentEventToChatMessage(
      second,
      outputEvent(1, 'stdout', 'stale first\n')
    )

    expect(staleFirst.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toEqual([
      { sequence: 2, stream: 'stdout', output: 'second\n' }
    ])
    expect(staleFirst.agentRun?.commandSessions?.['command-call']).toMatchObject({
      status: 'running',
      latestSequence: 2
    })
  })

  it('bounds tiny live chunks and marks the Session output as truncated', () => {
    const current = message()
    const bounded: ChatMessage = {
      ...current,
      agentRun: {
        ...current.agentRun!,
        commandSessions: {
          'command-call': {
            callId: 'command-call',
            sessionId: 'cmd_1234567890abcdef1234567890abcdef',
            status: 'running',
            latestSequence: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS,
            outputTruncated: false
          }
        },
        commandOutputPreviews: {
          'command-call': {
            callId: 'command-call',
            chunks: Array.from(
              { length: AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS },
              (_, index) => ({
                sequence: index + 1,
                stream: 'stdout' as const,
                output: 'x'
              })
            )
          }
        }
      }
    }

    const next = applyAgentEventToChatMessage(
      bounded,
      outputEvent(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS + 1, 'stdout', 'x')
    )
    const chunks = next.agentRun?.commandOutputPreviews?.['command-call']?.chunks ?? []
    expect(chunks).toHaveLength(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS)
    expect(chunks[0]?.sequence).toBe(2)
    expect(chunks.at(-1)?.sequence).toBe(AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS + 1)
    expect(next.agentRun?.commandSessions?.['command-call']?.outputTruncated).toBe(true)
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
        commandSessions: {
          'command-call': {
            callId: 'command-call',
            sessionId: 'cmd_1234567890abcdef1234567890abcdef',
            status: 'running',
            latestSequence: 1,
            outputTruncated: false
          }
        },
        commandOutputPreviews: {
          'command-call': {
            callId: 'command-call',
            chunks: [{ sequence: 1, stream: 'stdout', output: 'background output\n' }]
          }
        }
      }
    })
  })

  it('keeps a terminal process state after duplicate and late lifecycle events', () => {
    const started = applyAgentEventToChatMessage(message(), {
      type: 'command_started',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      startedAt: 10
    })
    const exited = applyAgentEventToChatMessage(started, {
      type: 'command_exited',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      status: 'exited',
      exitCode: 7,
      endedAt: 30,
      latestSequence: 2,
      outputTruncated: false
    })
    const lateOutput = applyAgentEventToChatMessage(
      exited,
      outputEvent(2, 'stderr', 'late duplicate')
    )
    const lateStarted = applyAgentEventToChatMessage(lateOutput, {
      type: 'command_started',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      startedAt: 10
    })

    expect(lateStarted.agentRun?.commandSessions?.['command-call']).toEqual({
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      status: 'exited',
      startedAt: 10,
      endedAt: 30,
      exitCode: 7,
      latestSequence: 2,
      outputTruncated: false
    })
    expect(lateStarted.agentRun?.commandOutputPreviews).toBeUndefined()
    expect(
      shouldTouchConversationForAgentEvent({
        type: 'command_exited',
        runId: 'run-command-output',
        conversationId: 'conversation-command-output',
        assistantMessageId: 'assistant-command-output',
        callId: 'command-call',
        sessionId: 'cmd_1234567890abcdef1234567890abcdef',
        status: 'exited',
        exitCode: 7,
        endedAt: 30,
        latestSequence: 2,
        outputTruncated: false
      })
    ).toBe(false)
  })

  it('settles only the pre-refresh Session identity missing from a successful Host list', () => {
    const oldSession = applyAgentEventToChatMessage(
      message(),
      outputEvent(1, 'stdout', 'old session\n')
    )
    const missing = markMissingAgentCommandSessionOutcomeUnknown(
      oldSession,
      'command-call',
      'cmd_1234567890abcdef1234567890abcdef',
      30
    )
    expect(missing.agentRun?.commandSessions?.['command-call']).toMatchObject({
      status: 'outcome_unknown',
      endedAt: 30
    })

    const newSession = applyAgentEventToChatMessage(message(), {
      ...outputEvent(1, 'stdout', 'new session\n'),
      sessionId: 'cmd_abcdef1234567890abcdef1234567890'
    })
    const staleReconciliation = markMissingAgentCommandSessionOutcomeUnknown(
      newSession,
      'command-call',
      'cmd_1234567890abcdef1234567890abcdef',
      30
    )
    expect(staleReconciliation).toBe(newSession)
    expect(staleReconciliation.agentRun?.commandSessions?.['command-call']?.status).toBe('running')
  })

  it('rejects lifecycle events for a different session bound to the same call', () => {
    const started = applyAgentEventToChatMessage(message(), {
      type: 'command_started',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      startedAt: 10
    })
    const wrongSessionOutput = applyAgentEventToChatMessage(started, {
      ...outputEvent(1, 'stdout', 'wrong session'),
      sessionId: 'cmd_abcdef1234567890abcdef1234567890'
    })

    expect(wrongSessionOutput).toEqual(started)
  })

  it('updates a completed message process without reopening its run', () => {
    const completed: ChatMessage = {
      ...message(),
      status: 'sent',
      agentRun: {
        ...message().agentRun!,
        status: 'completed',
        completedAt: 20,
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ]
      }
    }
    const next = applyAgentEventToChatMessage(completed, {
      type: 'command_interrupted',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      endedAt: 40,
      latestSequence: 0,
      outputTruncated: false
    })

    expect(next.status).toBe('sent')
    expect(next.agentRun?.status).toBe('completed')
    expect(next.agentRun?.commandSessions?.['command-call']?.status).toBe('interrupted')
  })

  it('settles approval immediately and projects the command as starting without a ToolResult', () => {
    const waiting: ChatMessage = {
      ...message(),
      agentRun: {
        ...message().agentRun!,
        status: 'waiting_for_approval',
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ],
        approvals: [
          {
            type: 'command',
            command: {
              id: 'command-call',
              command: 'printf first',
              approvalStatus: 'required'
            }
          }
        ]
      }
    }
    const approved = applyAgentActionExecutionToChatMessage(waiting, {
      actionId: 'command-call',
      actionType: 'command',
      toolName: 'run_command',
      status: 'approved',
      agentOutput: {
        status: 'running',
        content: '',
        runId: 'run-command-output',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    })

    expect(approved.agentRun?.approvals).toEqual([])
    expect(approved.agentRun?.toolCalls[0]?.approvalStatus).toBe('approved')
    expect(approved.agentRun?.commandSessions?.['command-call']?.status).toBe('starting')
    expect(approved.agentRun?.status).toBe('starting')
  })

  it('advances a waiting parent from Session authority and ignores a stale command approval', () => {
    const waiting: ChatMessage = {
      ...message(),
      agentRun: {
        ...message().agentRun!,
        status: 'waiting_for_approval',
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ],
        approvals: [
          {
            type: 'command',
            command: {
              id: 'command-call',
              command: 'printf first',
              approvalStatus: 'required'
            }
          }
        ]
      }
    }
    const started = applyAgentEventToChatMessage(waiting, {
      type: 'command_started',
      runId: 'run-command-output',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      callId: 'command-call',
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      startedAt: 10
    })

    expect(started.agentRun?.status).toBe('running')
    expect(started.agentRun?.approvals).toEqual([])
    expect(started.agentRun?.toolCalls[0]?.approvalStatus).toBe('approved')

    const staleApproval = applyAgentEventToChatMessage(started, {
      type: 'approval_required',
      runId: 'run-command-output',
      action: {
        type: 'command',
        command: {
          id: 'command-call',
          command: 'printf first',
          approvalStatus: 'required'
        }
      }
    })
    expect(staleApproval).toBe(started)
    expect(staleApproval.agentRun?.status).toBe('running')
    expect(staleApproval.agentRun?.approvals).toEqual([])
  })

  it('advances a reloaded waiting parent to starting from a Host Session snapshot', () => {
    const waiting: ChatMessage = {
      ...message(),
      agentRun: {
        ...message().agentRun!,
        status: 'waiting_for_approval',
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ],
        approvals: [
          {
            type: 'command',
            command: {
              id: 'command-call',
              command: 'printf first',
              approvalStatus: 'required'
            }
          }
        ]
      }
    }
    const restored = applyAgentCommandSessionSnapshotToChatMessage(waiting, {
      schemaVersion: 1,
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      originRunId: 'run-command-output',
      callId: 'command-call',
      command: 'printf first',
      cwd: '/workspace',
      commandDigest: `sha256:${'a'.repeat(64)}`,
      status: 'starting',
      startedAt: 10,
      latestSequence: 0,
      outputTruncated: false
    })

    expect(restored.agentRun?.status).toBe('starting')
    expect(restored.agentRun?.approvals).toEqual([])
    expect(restored.agentRun?.toolCalls[0]?.approvalStatus).toBe('approved')
  })

  it('does not reopen a completed Run when the approved action response arrives after done', () => {
    const completed: ChatMessage = {
      ...message(),
      content: 'The application is running.',
      status: 'sent',
      agentRun: {
        ...message().agentRun!,
        status: 'completed',
        completedAt: 40,
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ],
        approvals: [
          {
            type: 'command',
            command: {
              id: 'command-call',
              command: 'printf first',
              approvalStatus: 'required'
            }
          }
        ]
      }
    }

    const afterLateApproval = applyAgentActionExecutionToChatMessage(completed, {
      actionId: 'command-call',
      actionType: 'command',
      toolName: 'run_command',
      status: 'approved',
      agentOutput: {
        status: 'running',
        content: '',
        runId: 'run-command-output',
        events: [],
        toolDefinitions: [],
        proposedActions: []
      }
    })

    expect(afterLateApproval.content).toBe('The application is running.')
    expect(afterLateApproval.status).toBe('sent')
    expect(afterLateApproval.agentRun?.status).toBe('completed')
    expect(afterLateApproval.agentRun?.completedAt).toBe(40)
    expect(afterLateApproval.agentRun?.approvals).toEqual([])
    expect(afterLateApproval.agentRun?.toolCalls[0]?.approvalStatus).toBe('approved')
    expect(afterLateApproval.agentRun?.commandSessions?.['command-call']?.status).toBe('starting')
  })

  it('hydrates a bounded transcript without changing completed message state', () => {
    const completed: ChatMessage = {
      ...message(),
      status: 'sent',
      agentRun: {
        ...message().agentRun!,
        status: 'completed',
        completedAt: 20,
        toolCalls: [
          {
            ...message().agentRun!.toolCalls[0],
            approvalStatus: 'required'
          }
        ]
      }
    }
    const restored = applyAgentCommandSessionSnapshotToChatMessage(
      completed,
      {
        schemaVersion: 1,
        sessionId: 'cmd_1234567890abcdef1234567890abcdef',
        conversationId: 'conversation-command-output',
        assistantMessageId: 'assistant-command-output',
        originRunId: 'run-command-output',
        callId: 'command-call',
        command: 'printf first',
        cwd: '/workspace',
        commandDigest: `sha256:${'a'.repeat(64)}`,
        status: 'exited',
        startedAt: 10,
        endedAt: 30,
        exitCode: 0,
        latestSequence: 2,
        outputTruncated: false,
        outputs: [
          {
            name: 'reports/final.pdf',
            kind: 'document',
            readPath: `artifact://sha256/${'b'.repeat(64)}`,
            mimeType: 'application/pdf',
            sizeBytes: 4096,
            sha256: 'b'.repeat(64)
          }
        ]
      },
      {
        requestedAfterSequence: 0,
        firstAvailableSequence: 1,
        latestSequence: 2,
        truncatedBefore: false,
        outputCaptureTruncated: false,
        chunks: [
          { sequence: 1, stream: 'stdout', output: 'first\n' },
          { sequence: 2, stream: 'stdout', output: 'second\n' }
        ]
      }
    )

    expect(restored.status).toBe('sent')
    expect(restored.agentRun?.status).toBe('completed')
    expect(restored.agentRun?.toolCalls[0]?.approvalStatus).toBe('approved')
    expect(restored.agentRun?.commandSessions?.['command-call']?.status).toBe('exited')
    expect(restored.agentRun?.commandSessions?.['command-call']?.outputs?.[0]?.readPath).toBe(
      `artifact://sha256/${'b'.repeat(64)}`
    )
    expect(restored.agentRun?.commandOutputPreviews?.['command-call']?.chunks).toHaveLength(2)

    const stale = applyAgentCommandSessionSnapshotToChatMessage(restored, {
      schemaVersion: 1,
      sessionId: 'cmd_1234567890abcdef1234567890abcdef',
      conversationId: 'conversation-command-output',
      assistantMessageId: 'assistant-command-output',
      originRunId: 'run-command-output',
      callId: 'command-call',
      command: 'printf first',
      cwd: '/workspace',
      commandDigest: `sha256:${'a'.repeat(64)}`,
      status: 'running',
      startedAt: 10,
      latestSequence: 1,
      outputTruncated: false
    })
    expect(stale.agentRun?.commandSessions?.['command-call']?.status).toBe('exited')
  })
})
