import type { PendingAgentActionSnapshot } from '../agent'
import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import {
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentBuiltinMcpToolApprovalProposedAction
} from './builtinApprovals'
import { parseAgentMcpProposedAction } from './proposedActions'
import {
  expectBoundedNonEmptyString,
  expectExactString,
  expectModelToolCallId,
  expectOpaqueRunId,
  expectUuidV4,
  parseOptionalNullableString
} from './shared'
export function parsePendingAgentActionSnapshotsForHost(
  value: unknown
): PendingAgentActionSnapshot[] {
  if (!Array.isArray(value)) {
    throw invalidProtocolValue('pending Agent actions', 'expected an array')
  }
  if (value.length > 1024) {
    throw invalidProtocolValue('pending Agent actions', 'action count exceeded 1024')
  }
  return value.map((entry, index) => {
    const record = expectRecord(entry, `pending Agent actions[${index}]`)
    const action = expectRecord(record.action, `pending Agent actions[${index}].action`)
    if (
      action.type !== 'mcp_tool_call' &&
      action.type !== 'builtin_capability_activation' &&
      action.type !== 'builtin_mcp_tool_approval' &&
      action.type !== 'browser_risk_approval'
    ) {
      return entry as PendingAgentActionSnapshot
    }
    if (action.type === 'builtin_capability_activation') {
      const context = `pending built-in capability Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBuiltinCapabilityActivationProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (parsedAction.approval.actionId !== actionId || parsedAction.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'pending identity must match capability approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending capability approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match capability approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'builtin_capability_activation',
          `${context}.actionType`
        ),
        toolName: expectExactString(record.toolName, 'activate_capability', `${context}.toolName`),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    if (action.type === 'browser_risk_approval') {
      const context = `pending browser risk Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBrowserRiskProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (parsedAction.approval.actionId !== actionId || parsedAction.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'pending identity must match browser risk approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending browser risk approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match browser risk approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'browser_risk_approval',
          `${context}.actionType`
        ),
        toolName: expectExactString(
          record.toolName,
          parsedAction.approval.triggerToolName,
          `${context}.toolName`
        ),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    if (action.type === 'builtin_mcp_tool_approval') {
      const context = `pending built-in MCP Tool Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBuiltinMcpToolApprovalProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (
        parsedAction.approval.identity.actionId !== actionId ||
        parsedAction.approval.identity.runId !== runId
      ) {
        throw invalidProtocolValue(context, 'pending identity must match built-in MCP approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending built-in MCP approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.identity.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match built-in MCP approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'builtin_mcp_tool_approval',
          `${context}.actionType`
        ),
        toolName: expectExactString(
          record.toolName,
          parsedAction.approval.identity.rawName,
          `${context}.toolName`
        ),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    const context = `pending MCP Agent action[${index}]`
    expectOnlyKeys(
      record,
      [
        'actionId',
        'actionType',
        'toolName',
        'toolCallId',
        'runId',
        'conversationId',
        'assistantMessageId',
        'action',
        'createdAt',
        'status'
      ] as const,
      context
    )
    const parsedAction = parseAgentMcpProposedAction(action)
    const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
    const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
    if (
      parsedAction.approval.identity.actionId !== actionId ||
      parsedAction.approval.identity.runId !== runId
    ) {
      throw invalidProtocolValue(context, 'pending identity must match MCP approval')
    }
    const toolCallId = parseOptionalNullableString(record.toolCallId, `${context}.toolCallId`)
    if (toolCallId !== null && toolCallId !== parsedAction.approval.identity.callId) {
      throw invalidProtocolValue(context, 'toolCallId must match MCP approval')
    }
    return {
      actionId,
      actionType: expectExactString(record.actionType, 'mcp_tool_call', `${context}.actionType`),
      toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 64),
      toolCallId,
      runId,
      conversationId: parseOptionalNullableString(
        record.conversationId,
        `${context}.conversationId`
      ),
      assistantMessageId: parseOptionalNullableString(
        record.assistantMessageId,
        `${context}.assistantMessageId`
      ),
      action: parsedAction,
      createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
      status: expectEnum(
        record.status,
        [
          'pending',
          'approved',
          'executing',
          'rejected',
          'cancelled',
          'completed',
          'failed'
        ] as const,
        `${context}.status`
      )
    }
  })
}
