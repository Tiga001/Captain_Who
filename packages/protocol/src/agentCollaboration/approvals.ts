import type {
  CollaborationApprovalListRequest,
  CollaborationApprovalProjection,
  CollaborationApprovalList,
  CollaborationApprovalDecisionRequest,
  CollaborationApprovalDecisionResult
} from './types'
import { parseAgentTreeRequest } from './tree'
import { record, exact, text, integer, oneOf, schema, bool } from './validation'
import {
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentBrowserRiskProposedAction
} from '../agentParsers/builtinApprovals'
import { parsePendingAgentActionSnapshotsForHost } from '../agentParsers/pendingActions'

export function parseCollaborationApprovalListRequest(
  value: unknown
): CollaborationApprovalListRequest {
  return parseAgentTreeRequest(value)
}

export function parseCollaborationApprovalProjection(
  value: unknown
): CollaborationApprovalProjection {
  const item = record(value, 'CollaborationApprovalProjection')
  exact(
    item,
    [
      'schemaVersion',
      'approvalId',
      'rootAgentId',
      'rootConversationId',
      'sourceAgentId',
      'sourceTaskPath',
      'sourceConversationId',
      'runId',
      'actionId',
      'actionType',
      'toolName',
      'action',
      'status',
      'createdAt',
      'updatedAt'
    ],
    'CollaborationApprovalProjection'
  )
  const actionId = text(item.actionId, 'actionId')
  const actionType = text(item.actionType, 'actionType')
  const toolName = text(item.toolName, 'toolName')
  const runId = text(item.runId, 'runId', 2_048)
  const sourceConversationId = text(item.sourceConversationId, 'sourceConversationId')
  const createdAt = integer(item.createdAt, 'createdAt')
  const actionRecord = record(item.action, 'CollaborationApprovalProjection.action')
  const protectedToolCallId =
    actionRecord.type === 'builtin_capability_activation'
      ? parseAgentBuiltinCapabilityActivationProposedAction(actionRecord).approval.callId
      : actionRecord.type === 'browser_risk_approval'
        ? parseAgentBrowserRiskProposedAction(actionRecord).approval.callId
        : null
  const status = oneOf(
    item.status,
    [
      'pending',
      'approved',
      'executing',
      'rejected',
      'cancelled',
      'completed',
      'failed',
      'expired',
      'interrupted'
    ] as const,
    'status'
  )
  const parsedAction = parsePendingAgentActionSnapshotsForHost([
    {
      actionId,
      actionType,
      toolName,
      // Host-owned capability approvals are bound to the exact model Tool call. Passing null here
      // would make a collaboration projection weaker than the authoritative pending snapshot and
      // would also prevent the strict pending-action parser from hydrating it.
      toolCallId: protectedToolCallId,
      runId,
      conversationId: sourceConversationId,
      assistantMessageId: null,
      action: item.action,
      createdAt,
      status: status === 'expired' || status === 'interrupted' ? 'failed' : status
    }
  ])[0]?.action
  if (!parsedAction) throw new Error('Invalid approval action')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalProjection'),
    approvalId: text(item.approvalId, 'approvalId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    sourceAgentId: text(item.sourceAgentId, 'sourceAgentId'),
    sourceTaskPath: text(item.sourceTaskPath, 'sourceTaskPath', 4_096),
    sourceConversationId,
    runId,
    actionId,
    actionType,
    toolName,
    action: parsedAction,
    status,
    createdAt,
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseCollaborationApprovalList(value: unknown): CollaborationApprovalList {
  const item = record(value, 'CollaborationApprovalList')
  exact(item, ['schemaVersion', 'approvals'], 'CollaborationApprovalList')
  if (!Array.isArray(item.approvals) || item.approvals.length > 1_024)
    throw new Error('Invalid approvals')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalList'),
    approvals: item.approvals.map(parseCollaborationApprovalProjection)
  }
}

export function parseCollaborationApprovalDecisionRequest(
  value: unknown
): CollaborationApprovalDecisionRequest {
  const item = record(value, 'CollaborationApprovalDecisionRequest')
  exact(
    item,
    ['rootConversationId', 'approvalId', 'decision', 'message'],
    'CollaborationApprovalDecisionRequest'
  )
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    approvalId: text(item.approvalId, 'approvalId'),
    decision: oneOf(item.decision, ['approve', 'reject', 'cancel'] as const, 'decision'),
    message: item.message === null ? null : text(item.message, 'message', 4_096)
  }
}

export function parseCollaborationApprovalDecisionResult(
  value: unknown
): CollaborationApprovalDecisionResult {
  const item = record(value, 'CollaborationApprovalDecisionResult')
  exact(
    item,
    ['schemaVersion', 'approvalId', 'accepted', 'alreadySettled', 'status'],
    'CollaborationApprovalDecisionResult'
  )
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalDecisionResult'),
    approvalId: text(item.approvalId, 'approvalId'),
    accepted: bool(item.accepted, 'accepted'),
    alreadySettled: bool(item.alreadySettled, 'alreadySettled'),
    status: oneOf(
      item.status,
      [
        'pending',
        'approved',
        'executing',
        'rejected',
        'cancelled',
        'completed',
        'failed',
        'expired',
        'interrupted'
      ] as const,
      'status'
    )
  }
}
