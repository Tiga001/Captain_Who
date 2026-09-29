import type {
  AgentActionExecutionOutput,
  AgentEvent,
  AgentFileChangeResult,
  AgentProposedAction,
  AgentToolResult
} from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import {
  applyAgentActionExecutionToChatMessage,
  applyAgentEventToChatMessage
} from '../../features/agentRun/agentEventReducer'

const action: Extract<AgentProposedAction, { type: 'file_change' }> = {
  type: 'file_change',
  fileChange: {
    schemaVersion: 1,
    id: 'file-change-action-current',
    transactionId: 'file-change-transaction-current',
    operation: 'update',
    updateStrategy: 'rewrite',
    filePath: 'src/main.ts',
    inlineDiff: null,
    baseRevision: 'content-sha256-v1:base',
    summary: 'Update the entrypoint.',
    additions: 1,
    deletions: 1,
    lineCount: 1,
    byteCount: 4,
    approvalStatus: 'required'
  }
}

const result: AgentFileChangeResult = {
  schemaVersion: 1,
  status: 'applied',
  outcome: 'applied',
  transactionId: action.fileChange.transactionId,
  operation: 'update',
  updateStrategy: 'rewrite',
  filePath: 'src/main.ts',
  additions: 1,
  deletions: 1,
  lineCount: 1,
  byteCount: 4,
  revision: 'content-sha256-v1:target',
  errorCode: null,
  error: null,
  message: null
}

function waitingMessage(): ChatMessage {
  return {
    id: 'assistant-file-change',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun: {
      runId: 'run-file-change',
      status: 'waiting_for_approval',
      toolDefinitions: [],
      toolCalls: [
        {
          id: action.fileChange.id,
          tool: 'apply_patch',
          args: {
            request: {
              action: 'commit',
              transactionId: action.fileChange.transactionId,
              expectedDraftRevision: 1
            }
          },
          approvalStatus: 'required',
          reason: action.fileChange.summary
        }
      ],
      toolResults: [],
      approvals: [action],
      fileChangeProposals: [action.fileChange],
      fileChanges: [
        {
          schemaVersion: 1,
          transactionId: action.fileChange.transactionId,
          conversationId: 'conversation-file-change',
          projectId: 'project-file-change',
          filePath: action.fileChange.filePath,
          operation: action.fileChange.operation,
          updateStrategy: action.fileChange.updateStrategy,
          status: 'waiting_approval',
          baseRevision: action.fileChange.baseRevision,
          additions: 1,
          deletions: 1,
          lineCount: 1,
          byteCount: 4,
          mutationCount: 1,
          nextMutationIndex: 1,
          statsFinal: true,
          summary: action.fileChange.summary,
          createdAt: 1,
          updatedAt: 2
        }
      ],
      fileChangePreviews: [
        {
          schemaVersion: 1,
          previewId: 'preview-file-change',
          streamId: 'stream-file-change',
          attempt: 1,
          toolCallIndex: 0,
          toolCallId: action.fileChange.id,
          transactionId: action.fileChange.transactionId,
          filePath: action.fileChange.filePath,
          additions: 1,
          deletions: 1,
          lineCount: 1,
          byteCount: 4,
          generatedBytes: 4,
          updatedAt: 2,
          content: 'safe transient preview',
          receivedAt: 2
        }
      ],
      timeline: []
    }
  }
}

function execution(
  overrides: Partial<AgentActionExecutionOutput> = {}
): AgentActionExecutionOutput {
  return {
    actionId: action.fileChange.id,
    actionType: 'file_change',
    toolName: 'apply_patch',
    status: 'applied',
    fileChangeResult: result,
    agentOutput: {
      content: '',
      status: 'running',
      runId: 'run-file-change',
      events: [],
      toolDefinitions: [],
      proposedActions: []
    },
    ...overrides
  }
}

function notificationToolResult(): AgentToolResult {
  return {
    callId: action.fileChange.id,
    tool: 'apply_patch',
    ok: true,
    result
  }
}

function projectedSettlement(message: ChatMessage) {
  return {
    status: message.agentRun?.status,
    approvals: message.agentRun?.approvals,
    toolCalls: message.agentRun?.toolCalls,
    toolResults: message.agentRun?.toolResults,
    fileChangeProposals: message.agentRun?.fileChangeProposals,
    fileChanges: message.agentRun?.fileChanges?.map((fileChange) => ({
      ...fileChange,
      updatedAt: 0
    })),
    fileChangePreviews: message.agentRun?.fileChangePreviews
  }
}

describe('FileChange action execution projection', () => {
  it('settles a FileChange from the authoritative fileChangeResult-only RPC response', () => {
    const settled = applyAgentActionExecutionToChatMessage(
      waitingMessage(),
      execution(),
      undefined,
      action
    )

    expect(settled.agentRun?.status).toBe('running')
    expect(settled.agentRun?.approvals).toEqual([])
    expect(settled.agentRun?.toolCalls).toContainEqual(
      expect.objectContaining({ id: action.fileChange.id, approvalStatus: 'approved' })
    )
    expect(settled.agentRun?.fileChangeProposals).toContainEqual(
      expect.objectContaining({ id: action.fileChange.id, approvalStatus: 'approved' })
    )
    expect(settled.agentRun?.fileChanges).toContainEqual(
      expect.objectContaining({
        transactionId: action.fileChange.transactionId,
        status: 'applied'
      })
    )
    expect(settled.agentRun?.toolResults).toEqual([notificationToolResult()])
    expect(settled.agentRun?.fileChangePreviews).toEqual([])
  })

  it('retains a newer approval when the earlier file decision RPC arrives after its continuation', () => {
    const nextAction: Extract<AgentProposedAction, { type: 'file_change' }> = {
      type: 'file_change',
      fileChange: {
        ...action.fileChange,
        id: 'file-change-action-next',
        transactionId: 'file-change-transaction-next',
        summary: 'Fix the next assertion.'
      }
    }
    const newerUsage = { totalTokens: 30, billableRequestCount: 3 }
    const events: AgentEvent[] = [
      { type: 'tool_result', runId: 'run-file-change', result: notificationToolResult() },
      { type: 'approval_required', runId: 'run-file-change', action: nextAction },
      {
        type: 'state',
        runId: 'run-file-change',
        state: {
          status: 'waiting_for_approval',
          activeRunId: 'run-file-change',
          lastError: null,
          updatedAt: 3
        }
      },
      {
        type: 'done',
        runId: 'run-file-change',
        status: 'waiting_for_approval',
        success: true,
        proposedActions: [nextAction],
        usage: newerUsage
      }
    ]
    const waitingForNextAction = events.reduce(applyAgentEventToChatMessage, waitingMessage())
    const lateExecution = execution({
      agentOutput: {
        ...execution().agentOutput,
        usage: { totalTokens: 10, billableRequestCount: 1 }
      }
    })
    const settled = applyAgentActionExecutionToChatMessage(
      waitingForNextAction,
      lateExecution,
      undefined,
      action
    )

    expect(settled.agentRun?.status).toBe('waiting_for_approval')
    expect(settled.agentRun?.state?.status).toBe('waiting_for_approval')
    expect(settled.agentRun?.approvals).toEqual([nextAction])
    expect(settled.agentRun?.usage).toEqual(newerUsage)
    expect(settled.agentRun?.toolResults).toContainEqual(notificationToolResult())
    expect(settled.agentRun?.toolCalls).toContainEqual(
      expect.objectContaining({ id: action.fileChange.id, approvalStatus: 'approved' })
    )
    expect(settled.agentRun?.toolCalls).toContainEqual(
      expect.objectContaining({ id: nextAction.fileChange.id, approvalStatus: 'required' })
    )
    expect(
      projectedSettlement(
        applyAgentActionExecutionToChatMessage(settled, lateExecution, undefined, action)
      )
    ).toEqual(projectedSettlement(settled))
  })

  it('ignores an approval RPC belonging to a different Run', () => {
    const current = waitingMessage()
    current.agentRun!.runId = 'run-other'

    expect(applyAgentActionExecutionToChatMessage(current, execution(), undefined, action)).toBe(
      current
    )
  })

  it.each(['acknowledgement', 'running receipt'])(
    'preserves file approval B and a settled command when command A returns a late %s',
    (responseKind) => {
      const waiting = waitingMessage()
      const run = waiting.agentRun!
      run.toolCalls.push({
        id: 'command-earlier',
        tool: 'run_command',
        args: { command: 'python3 binary_tree.py' },
        approvalStatus: 'required',
        reason: 'Run the assertions.'
      })
      const settledSession = {
        callId: 'command-earlier',
        sessionId: 'command-session-earlier',
        status: 'exited' as const,
        startedAt: 1,
        endedAt: 2,
        exitCode: 0,
        latestSequence: 3,
        outputTruncated: false
      }
      run.commandSessions = { 'command-earlier': settledSession }
      const settled = applyAgentActionExecutionToChatMessage(waiting, {
        actionId: 'command-earlier',
        actionType: 'command',
        toolName: 'run_command',
        status: 'applied',
        agentOutput: execution().agentOutput,
        ...(responseKind === 'running receipt'
          ? {
              toolResult: {
                callId: 'command-earlier',
                tool: 'run_command',
                ok: true,
                result: {
                  status: 'running',
                  sessionId: settledSession.sessionId,
                  startedAt: 1,
                  latestSequence: 0
                }
              }
            }
          : {})
      })

      expect(settled.agentRun?.status).toBe('waiting_for_approval')
      expect(settled.agentRun?.approvals).toEqual([action])
      expect(settled.agentRun?.commandSessions?.['command-earlier']).toEqual(settledSession)
      expect(settled.agentRun?.toolCalls).toContainEqual(
        expect.objectContaining({ id: 'command-earlier', approvalStatus: 'approved' })
      )
      const lateStarted = applyAgentEventToChatMessage(settled, {
        type: 'command_started',
        runId: 'run-file-change',
        conversationId: 'conversation-file-change',
        assistantMessageId: waiting.id,
        callId: 'command-earlier',
        sessionId: settledSession.sessionId,
        startedAt: 1
      })
      expect(lateStarted.agentRun?.status).toBe('waiting_for_approval')
      expect(lateStarted.agentRun?.approvals).toEqual([action])
      expect(lateStarted.agentRun?.commandSessions?.['command-earlier']).toEqual(settledSession)
    }
  )

  it('settles the file action without reopening a completed Run on a late RPC', () => {
    const completed = applyAgentEventToChatMessage(waitingMessage(), {
      type: 'done',
      runId: 'run-file-change',
      status: 'completed',
      success: true,
      content: 'The task is complete.',
      usage: { totalTokens: 30, billableRequestCount: 3 }
    })
    const settled = applyAgentActionExecutionToChatMessage(
      completed,
      execution(),
      undefined,
      action
    )

    expect(settled.status).toBe('sent')
    expect(settled.content).toBe('The task is complete.')
    expect(settled.agentRun?.status).toBe('completed')
    expect(settled.agentRun?.usage).toEqual(completed.agentRun?.usage)
    expect(settled.agentRun?.toolResults).toContainEqual(notificationToolResult())
  })

  it('converges idempotently when the ToolResult notification arrives before or after the RPC', () => {
    const event: AgentEvent = {
      type: 'tool_result',
      runId: 'run-file-change',
      result: notificationToolResult()
    }

    const notificationFirst = applyAgentActionExecutionToChatMessage(
      applyAgentEventToChatMessage(waitingMessage(), event),
      execution(),
      undefined,
      action
    )
    const rpcFirst = applyAgentEventToChatMessage(
      applyAgentActionExecutionToChatMessage(waitingMessage(), execution(), undefined, action),
      event
    )
    const replayed = applyAgentEventToChatMessage(rpcFirst, event)

    expect(projectedSettlement(notificationFirst)).toEqual(projectedSettlement(rpcFirst))
    expect(projectedSettlement(replayed)).toEqual(projectedSettlement(rpcFirst))
    expect(replayed.agentRun?.toolResults).toHaveLength(1)
  })

  it.each([
    {
      name: 'action id',
      execution: execution({ actionId: 'file-change-action-forged' }),
      decidedAction: action
    },
    {
      name: 'transaction id',
      execution: execution({
        fileChangeResult: { ...result, transactionId: 'file-change-transaction-forged' }
      }),
      decidedAction: action
    },
    {
      name: 'missing exact action context',
      execution: execution(),
      decidedAction: undefined
    }
  ])('fails closed on a mismatched $name', ({ execution: value, decidedAction }) => {
    const current = waitingMessage()

    expect(() =>
      applyAgentActionExecutionToChatMessage(current, value, undefined, decidedAction)
    ).toThrow('Invalid FileChange action execution identity')
    expect(current).toEqual(waitingMessage())
  })

  it('projects a rejected FileChange as a successful no-effect protocol settlement', () => {
    const rejectedResult: AgentFileChangeResult = {
      ...result,
      status: 'rejected',
      outcome: 'definitely_not_executed',
      revision: null,
      errorCode: null,
      message: 'The file change was rejected.'
    }
    const settled = applyAgentActionExecutionToChatMessage(
      waitingMessage(),
      execution({ status: 'rejected', fileChangeResult: rejectedResult }),
      undefined,
      action
    )

    expect(settled.agentRun?.toolResults).toEqual([
      expect.objectContaining({
        callId: action.fileChange.id,
        tool: 'apply_patch',
        ok: true,
        result: rejectedResult
      })
    ])
    expect(settled.agentRun?.fileChanges).toContainEqual(
      expect.objectContaining({ status: 'rejected' })
    )
    expect(settled.agentRun?.fileChangeProposals).toContainEqual(
      expect.objectContaining({ approvalStatus: 'rejected' })
    )
  })
})
