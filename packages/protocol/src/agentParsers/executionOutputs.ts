import type { AgentActionExecutionOutput } from '../agent'
import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  invalidProtocolValue
} from '../skills/validation'
import { parseMcpAgentChatOutput } from './agentOutput'
import { BROWSER_REVIEWED_TOOL_NAMES, BUILTIN_MCP_APPROVAL_TOOL_NAMES } from './builtinApprovals'
import { parseAgentFileChangeResult } from './fileChanges'
import {
  expectBoundedNonEmptyString,
  expectExactString,
  expectOpaqueRunId,
  expectUuidV4
} from './shared'
/**
 * MCP action execution has a dedicated safe projection. Generic Tool result bodies, exact trace
 * records and Tool definitions are intentionally not forwarded to Renderer.
 */
export function parseAgentActionExecutionOutputForHost(value: unknown): AgentActionExecutionOutput {
  const record = expectRecord(value, 'Agent action execution output')
  expectOnlyKeys(
    record,
    [
      'actionId',
      'actionType',
      'toolName',
      'status',
      'fileChangeResult',
      'commandResult',
      'toolResult',
      'agentOutput'
    ] as const,
    'Agent action execution output'
  )
  if (record.actionType === 'file_change') {
    if (record.commandResult !== undefined || record.toolResult !== undefined) {
      throw invalidProtocolValue(
        'FileChange action execution output',
        'commandResult and toolResult are forbidden for file_change'
      )
    }
    const status = expectEnum(
      record.status,
      [
        'applied',
        'failed',
        'conflict',
        'rejected',
        'cancelled',
        'expired',
        'outcome_unknown'
      ] as const,
      'FileChange action execution output.status'
    )
    const fileChangeResult = parseAgentFileChangeResult(
      record.fileChangeResult,
      'FileChange action execution output.fileChangeResult'
    )
    const matchingResultStatuses = {
      applied: ['applied', 'already_applied'],
      failed: ['failed'],
      conflict: ['conflict'],
      rejected: ['rejected'],
      cancelled: ['aborted'],
      expired: ['expired'],
      outcome_unknown: ['outcome_unknown']
    } as const
    if (!(matchingResultStatuses[status] as readonly string[]).includes(fileChangeResult.status)) {
      throw invalidProtocolValue(
        'FileChange action execution output',
        'status does not match fileChangeResult.status'
      )
    }
    return {
      actionId: expectOpaqueRunId(record.actionId, 'FileChange action execution output.actionId'),
      actionType: expectExactString(
        record.actionType,
        'file_change',
        'FileChange action execution output.actionType'
      ),
      toolName: expectExactString(
        record.toolName,
        'apply_patch',
        'FileChange action execution output.toolName'
      ),
      status,
      fileChangeResult,
      agentOutput: parseMcpAgentChatOutput(
        record.agentOutput,
        'FileChange action execution output.agentOutput'
      )
    }
  }
  if (record.fileChangeResult !== undefined) {
    throw invalidProtocolValue(
      'Agent action execution output',
      'fileChangeResult is only valid for file_change'
    )
  }
  if (
    record.actionType !== 'mcp_tool_call' &&
    record.actionType !== 'builtin_capability_activation' &&
    record.actionType !== 'builtin_mcp_tool_approval' &&
    record.actionType !== 'browser_risk_approval'
  ) {
    return value as AgentActionExecutionOutput
  }
  const builtinActivation = record.actionType === 'builtin_capability_activation'
  const builtinMcpToolApproval = record.actionType === 'builtin_mcp_tool_approval'
  const browserRisk = record.actionType === 'browser_risk_approval'
  const context = builtinActivation
    ? 'built-in capability Agent action execution output'
    : builtinMcpToolApproval
      ? 'built-in MCP Tool Agent action execution output'
      : browserRisk
        ? 'browser risk Agent action execution output'
        : 'MCP Agent action execution output'
  return {
    actionId: expectUuidV4(record.actionId, `${context}.actionId`),
    actionType: expectExactString(
      record.actionType,
      builtinActivation
        ? 'builtin_capability_activation'
        : builtinMcpToolApproval
          ? 'builtin_mcp_tool_approval'
          : browserRisk
            ? 'browser_risk_approval'
            : 'mcp_tool_call',
      `${context}.actionType`
    ),
    toolName: builtinActivation
      ? expectExactString(record.toolName, 'activate_capability', `${context}.toolName`)
      : builtinMcpToolApproval
        ? expectEnum(record.toolName, BUILTIN_MCP_APPROVAL_TOOL_NAMES, `${context}.toolName`)
        : browserRisk
          ? expectEnum(record.toolName, BROWSER_REVIEWED_TOOL_NAMES, `${context}.toolName`)
          : expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 64),
    status: expectEnum(
      record.status,
      ['applied', 'approved', 'failed', 'conflict', 'rejected'] as const,
      `${context}.status`
    ),
    agentOutput: parseMcpAgentChatOutput(record.agentOutput, `${context}.agentOutput`)
  }
}
