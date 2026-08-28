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
            action: 'commit',
            transactionId: action.fileChange.transactionId,
            expectedDraftRevision: 1
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
