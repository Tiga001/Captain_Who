import type {
  AgentApprovalStatus,
  AgentFileChangeStatus,
  AgentProposedAction,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import type { ChatAgentRunView } from '../chat/chatTypes'
import { getAgentActionId } from './agentActionUtils'
import { getActionToolCall, withActionApprovalStatus } from './actionProjection'

function upsertById<T>(items: T[], nextItem: T, getId: (item: T) => string) {
  const nextId = getId(nextItem)
  const itemIndex = items.findIndex((item) => getId(item) === nextId)
  if (itemIndex === -1) return [...items, nextItem]
  return items.map((item, index) => (index === itemIndex ? nextItem : item))
}

export function updateToolCallApprovalStatus(
  toolCalls: AgentToolCall[],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type === 'mcp_tool_call') {
    return toolCalls.filter((call) => call.id !== action.approval.identity.callId)
  }

  const actionId = getAgentActionId(action)
  const actionCall = getActionToolCall(withActionApprovalStatus(action, approvalStatus))

  if (!actionCall) return toolCalls
  return upsertById(
    toolCalls.map((call) => (call.id === actionId ? { ...call, approvalStatus } : call)),
    actionCall,
    (call) => call.id
  )
}

export function updateDiffApprovalStatus(
  diffs: ChatAgentRunView['diffs'],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type !== 'file_change') return diffs
  return upsertById(
    diffs.map((diff) => (diff.id === action.fileChange.id ? { ...diff, approvalStatus } : diff)),
    { ...action.fileChange, approvalStatus },
    (diff) => diff.id
  )
}

export function updateFileDraftApprovalStatus(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type !== 'file_change' || action.fileChange.inlineDiff !== null) return fileDrafts
  const fileChange = action.fileChange
  const status: AgentFileChangeStatus =
    approvalStatus === 'required'
      ? 'waiting_approval'
      : approvalStatus === 'rejected'
        ? 'rejected'
        : 'applying'
  const updated = fileDrafts.map((draft) =>
    draft.transactionId === fileChange.transactionId
      ? { ...draft, status, statsFinal: true, updatedAt: Date.now() }
      : draft
  )
  if (updated.some((draft) => draft.transactionId === fileChange.transactionId)) return updated
  const now = Date.now()
  return [
    ...updated,
    {
      schemaVersion: 1 as const,
      transactionId: fileChange.transactionId,
      conversationId: '',
      projectId: null,
      filePath: fileChange.filePath,
      operation: fileChange.operation,
      updateStrategy: fileChange.updateStrategy,
      status,
      baseRevision: fileChange.baseRevision,
      additions: fileChange.additions,
      deletions: fileChange.deletions,
      lineCount: fileChange.lineCount,
      byteCount: fileChange.byteCount,
      mutationCount: 0,
      nextMutationIndex: 0,
      statsFinal: true,
      summary: fileChange.summary,
      createdAt: now,
      updatedAt: now
    }
  ]
}

export function updateFileDraftFromToolResult(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  result: AgentToolResult
) {
  if (result.tool !== 'apply_patch' || !result.result || typeof result.result !== 'object') {
    return fileDrafts
  }
  const value = result.result as Record<string, unknown>
  const transactionId = typeof value.transactionId === 'string' ? value.transactionId : ''
  if (!transactionId) return fileDrafts
  return fileDrafts.map((draft) => {
    if (draft.transactionId !== transactionId) return draft
    const resultStatus = value.status
    const status: AgentFileChangeStatus =
      resultStatus === 'applied' || resultStatus === 'already_applied'
        ? 'applied'
        : resultStatus === 'conflict'
          ? 'conflict'
          : resultStatus === 'rejected'
            ? 'rejected'
            : resultStatus === 'failed'
              ? 'failed'
              : draft.status
    return {
      ...draft,
      status,
      additions: typeof value.additions === 'number' ? value.additions : draft.additions,
      deletions: typeof value.deletions === 'number' ? value.deletions : draft.deletions,
      lineCount: typeof value.lineCount === 'number' ? value.lineCount : draft.lineCount,
      byteCount: typeof value.byteCount === 'number' ? value.byteCount : draft.byteCount,
      statsFinal: true,
      updatedAt: Date.now()
    }
  })
}

export function createRejectedToolResult(
  action: AgentProposedAction,
  message?: string
): AgentToolResult | null {
  if (action.type === 'mcp_tool_call') return null

  const call = getActionToolCall(action)
  if (!call) return null

  if (action.type === 'file_change') {
    const fileChange = action.fileChange
    return {
      callId: call.id,
      tool: 'apply_patch',
      ok: true,
      result: {
        schemaVersion: 1,
        status: 'rejected',
        outcome: 'definitely_not_executed',
        transactionId: fileChange.transactionId,
        operation: fileChange.operation,
        updateStrategy: fileChange.updateStrategy,
        filePath: fileChange.filePath,
        additions: fileChange.additions,
        deletions: fileChange.deletions,
        lineCount: fileChange.lineCount,
        byteCount: fileChange.byteCount,
        revision: null,
        errorCode: 'rejected',
        error: null,
        message: message ?? '文件修改已拒绝。'
      }
    }
  }

  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: { status: 'rejected', message }
  }
}

export function getApprovalsForStatus(
  status: ChatAgentRunView['status'],
  currentApprovals: AgentProposedAction[],
  proposedActions: AgentProposedAction[]
) {
  if (status !== 'waiting_for_approval') return []
  return proposedActions.reduce(
    (actions, action) => upsertById(actions, action, getAgentActionId),
    currentApprovals
  )
}
