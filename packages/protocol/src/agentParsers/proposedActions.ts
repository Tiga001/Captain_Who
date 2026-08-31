import type { AgentProposedAction } from '../agent'
import { expectOnlyKeys, expectRecord, invalidProtocolValue } from '../skills/validation'
import {
  parseAgentCommandAction,
  parseAgentOfficeOperationRequest,
  parseAgentSkillInstallationRequest,
  parseAgentSkillMaterializationRequest,
  parseAgentSkillScriptRequest
} from './actionPayloads'
import {
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentBuiltinMcpToolApprovalProposedAction
} from './builtinApprovals'
import { parseAgentToolCallForHost } from './eventPayloads'
import { parseAgentFileChangeProposal } from './fileChanges'
import { parseAgentMcpToolApproval } from './mcpIdentity'
import {
  MAX_RENDERER_SAFE_PROPOSED_ACTIONS,
  expectBoundedArray,
  expectBoundedNonEmptyString
} from './shared'
export function parseAgentProposedActionsForHost(
  value: unknown,
  context: string,
  runId: string
): AgentProposedAction[] {
  return expectBoundedArray(value, context, MAX_RENDERER_SAFE_PROPOSED_ACTIONS).map(
    (action, index) => parseAgentProposedActionForHost(action, `${context}[${index}]`, runId, false)
  )
}

export function parseAgentProposedActionForHost(
  value: unknown,
  context: string,
  runId: string,
  requireApproval: boolean
): AgentProposedAction {
  const item = expectRecord(value, context)
  const type = expectBoundedNonEmptyString(item.type, `${context}.type`, 128)
  let action: AgentProposedAction
  switch (type) {
    case 'tool_call':
      expectOnlyKeys(item, ['type', 'call'] as const, context)
      action = { type, call: parseAgentToolCallForHost(item.call, `${context}.call`) }
      break
    case 'mcp_tool_call':
      action = parseAgentMcpProposedAction(item)
      if (action.approval.identity.runId !== runId) {
        throw invalidProtocolValue(context, 'approval identity runId must match enclosing runId')
      }
      break
    case 'builtin_capability_activation':
      action = parseAgentBuiltinCapabilityActivationProposedAction(item)
      if (action.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'approval runId must match enclosing runId')
      }
      break
    case 'builtin_mcp_tool_approval':
      action = parseAgentBuiltinMcpToolApprovalProposedAction(item)
      if (action.approval.identity.runId !== runId) {
        throw invalidProtocolValue(context, 'approval identity runId must match enclosing runId')
      }
      break
    case 'browser_risk_approval':
      action = parseAgentBrowserRiskProposedAction(item)
      if (action.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'approval runId must match enclosing runId')
      }
      break
    case 'file_change':
      expectOnlyKeys(item, ['type', 'fileChange'] as const, context)
      action = {
        type,
        fileChange: parseAgentFileChangeProposal(item.fileChange, `${context}.fileChange`)
      }
      break
    case 'command':
      expectOnlyKeys(item, ['type', 'command'] as const, context)
      action = { type, command: parseAgentCommandAction(item.command, `${context}.command`) }
      break
    case 'skill_materialization':
      expectOnlyKeys(item, ['type', 'materialization'] as const, context)
      action = {
        type,
        materialization: parseAgentSkillMaterializationRequest(
          item.materialization,
          `${context}.materialization`
        )
      }
      break
    case 'skill_script':
      expectOnlyKeys(item, ['type', 'script'] as const, context)
      action = { type, script: parseAgentSkillScriptRequest(item.script, `${context}.script`) }
      break
    case 'office_operation':
      expectOnlyKeys(item, ['type', 'officeOperation'] as const, context)
      action = {
        type,
        officeOperation: parseAgentOfficeOperationRequest(
          item.officeOperation,
          `${context}.officeOperation`
        )
      }
      break
    case 'skill_installation':
      expectOnlyKeys(item, ['type', 'installation'] as const, context)
      action = {
        type,
        installation: parseAgentSkillInstallationRequest(
          item.installation,
          `${context}.installation`
        )
      }
      break
    default:
      throw invalidProtocolValue(`${context}.type`, `unknown proposed action type ${type}`)
  }

  if (requireApproval && proposedActionApprovalStatus(action) !== 'required') {
    throw invalidProtocolValue(context, 'approval_required action must remain required')
  }
  return action
}

export function proposedActionApprovalStatus(action: AgentProposedAction) {
  switch (action.type) {
    case 'tool_call':
      return action.call.approvalStatus
    case 'mcp_tool_call':
      return action.approval.call.approvalStatus
    case 'builtin_capability_activation':
    case 'builtin_mcp_tool_approval':
    case 'browser_risk_approval':
      return action.approval.approvalStatus
    case 'file_change':
      return action.fileChange.approvalStatus
    case 'command':
      return action.command.approvalStatus
    case 'skill_materialization':
      return action.materialization.approvalStatus
    case 'skill_script':
      return action.script.approvalStatus
    case 'office_operation':
      return action.officeOperation.approvalStatus
    case 'skill_installation':
      return action.installation.approvalStatus
  }
}

export function parseAgentMcpProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'mcp_tool_call' }> {
  const context = 'MCP proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'mcp_tool_call') {
    throw invalidProtocolValue(context, 'type must be mcp_tool_call')
  }
  return {
    type: 'mcp_tool_call',
    approval: parseAgentMcpToolApproval(record.approval)
  }
}

export function parseStrictProposedActions(
  value: unknown,
  context: string,
  enclosingRunId: string
): Extract<
  AgentProposedAction,
  {
    type:
      | 'mcp_tool_call'
      | 'builtin_capability_activation'
      | 'builtin_mcp_tool_approval'
      | 'browser_risk_approval'
  }
>[] {
  if (!Array.isArray(value) || value.length > MAX_RENDERER_SAFE_PROPOSED_ACTIONS) {
    throw invalidProtocolValue(
      context,
      `must be an array with at most ${MAX_RENDERER_SAFE_PROPOSED_ACTIONS} items`
    )
  }
  return value.map((action, index) => {
    const actionRecord = expectRecord(action, `${context}[${index}]`)
    if (
      actionRecord.type !== 'mcp_tool_call' &&
      actionRecord.type !== 'builtin_capability_activation' &&
      actionRecord.type !== 'builtin_mcp_tool_approval' &&
      actionRecord.type !== 'browser_risk_approval'
    ) {
      throw invalidProtocolValue(
        context,
        'mixed MCP and non-MCP or protected and generic proposed actions are not supported by this safe projection'
      )
    }
    const parsed =
      actionRecord.type === 'mcp_tool_call'
        ? parseAgentMcpProposedAction(actionRecord)
        : actionRecord.type === 'builtin_capability_activation'
          ? parseAgentBuiltinCapabilityActivationProposedAction(actionRecord)
          : actionRecord.type === 'builtin_mcp_tool_approval'
            ? parseAgentBuiltinMcpToolApprovalProposedAction(actionRecord)
            : parseAgentBrowserRiskProposedAction(actionRecord)
    if (
      (parsed.type === 'builtin_capability_activation' ||
        parsed.type === 'builtin_mcp_tool_approval' ||
        parsed.type === 'browser_risk_approval') &&
      parsed.approval.approvalStatus !== 'required'
    ) {
      throw invalidProtocolValue(context, 'proposed protected approval must remain required')
    }
    const approvalRunId =
      parsed.type === 'mcp_tool_call' || parsed.type === 'builtin_mcp_tool_approval'
        ? parsed.approval.identity.runId
        : parsed.approval.runId
    if (approvalRunId !== enclosingRunId) {
      throw invalidProtocolValue(context, 'approval identity runId must match enclosing runId')
    }
    return parsed
  })
}
