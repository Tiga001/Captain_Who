import type {
  AgentApprovalStatus,
  AgentFileDraftStatus,
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
  if (action.type !== 'diff') return diffs
  return upsertById(
    diffs.map((diff) => (diff.id === action.diff.id ? { ...diff, approvalStatus } : diff)),
    { ...action.diff, approvalStatus },
    (diff) => diff.id
  )
}

export function updateFileDraftApprovalStatus(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
) {
  if (action.type !== 'file_write') return fileDrafts
  const status: AgentFileDraftStatus =
    approvalStatus === 'required'
      ? 'waiting_approval'
      : approvalStatus === 'rejected'
        ? 'rejected'
        : 'applying'
  const updated = fileDrafts.map((draft) =>
    draft.draftId === action.fileWrite.draftId
      ? { ...draft, status, statsFinal: true, updatedAt: Date.now() }
      : draft
  )
  if (updated.some((draft) => draft.draftId === action.fileWrite.draftId)) return updated
  const now = Date.now()
  return [
    ...updated,
    {
      draftId: action.fileWrite.draftId,
      conversationId: '',
      filePath: action.fileWrite.filePath,
      mode: action.fileWrite.mode,
      status,
      baseRevision: action.fileWrite.baseRevision,
      additions: action.fileWrite.additions,
      deletions: action.fileWrite.deletions,
      lineCount: action.fileWrite.lineCount,
      byteCount: action.fileWrite.byteCount,
      chunkCount: 0,
      nextChunkIndex: 0,
      statsFinal: true,
      summary: action.fileWrite.summary,
      createdAt: now,
      updatedAt: now
    }
  ]
}

export function updateFileDraftFromToolResult(
  fileDrafts: NonNullable<ChatAgentRunView['fileDrafts']>,
  result: AgentToolResult
) {
  if (result.tool !== 'write_file' || !result.result || typeof result.result !== 'object') {
    return fileDrafts
  }
  const value = result.result as Record<string, unknown>
  const draftId = typeof value.draftId === 'string' ? value.draftId : ''
  if (!draftId) return fileDrafts
  return fileDrafts.map((draft) => {
    if (draft.draftId !== draftId) return draft
    const resultStatus = value.status
    const status: AgentFileDraftStatus =
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
  const call = getActionToolCall(action)
  if (!call) return null

  if (action.type === 'diff') {
    return {
      callId: call.id,
      tool: 'apply_patch',
      ok: true,
      result: {
        status: 'rejected',
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        appliedFilePaths: [],
        message
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
