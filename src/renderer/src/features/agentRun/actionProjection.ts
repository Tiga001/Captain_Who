import type { AgentApprovalStatus, AgentProposedAction, AgentToolCall } from '@mycopilot/protocol'

export function getActionToolCall(action: AgentProposedAction): AgentToolCall | null {
  if (action.type === 'tool_call') return action.call

  if (action.type === 'builtin_capability_activation') {
    return {
      id: action.approval.callId,
      tool: 'activate_capability',
      args: {},
      approvalStatus: action.approval.approvalStatus,
      reason: action.approval.reason
    }
  }

  if (action.type === 'diff') {
    return {
      id: action.diff.id,
      tool: 'apply_patch',
      args: {
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        patch: action.diff.patch,
        summary: action.diff.summary
      },
      approvalStatus: action.diff.approvalStatus,
      reason: action.diff.summary
    }
  }

  if (action.type === 'file_write') {
    return {
      id: action.fileWrite.id,
      tool: 'write_file',
      args: {
        phase: 'finish',
        draftId: action.fileWrite.draftId,
        summary: action.fileWrite.summary
      },
      approvalStatus: action.fileWrite.approvalStatus,
      reason: action.fileWrite.summary
    }
  }

  if (action.type === 'skill_materialization') {
    const args: Record<string, unknown> = {
      sourceUri: action.materialization.sourceUri,
      destination: action.materialization.destination
    }
    copyIfDefined(args, 'sourcePrefix', action.materialization.sourcePrefix)
    copyIfDefined(args, 'reason', action.materialization.reason)
    return {
      id: action.materialization.id,
      tool: 'skills_materialize_resource',
      args,
      approvalStatus: action.materialization.approvalStatus,
      reason: action.materialization.reason
    }
  }

  if (action.type === 'skill_script') {
    const args: Record<string, unknown> = {
      scriptUri: action.script.scriptUri,
      interpreter: action.script.interpreter,
      args: action.script.args,
      requirements: action.script.requirements
    }
    copyIfDefined(args, 'timeoutMs', action.script.timeoutMs)
    copyIfDefined(args, 'reason', action.script.reason)
    return {
      id: action.script.id,
      tool: 'skills_run_script',
      args,
      approvalStatus: action.script.approvalStatus,
      reason: action.script.reason
    }
  }

  if (action.type === 'office_operation') {
    const request = action.officeOperation.prepared.request
    const tool =
      request.documentKind === 'document'
        ? 'office_document'
        : request.documentKind === 'spreadsheet'
          ? 'office_spreadsheet'
          : 'office_presentation'
    return {
      id: action.officeOperation.id,
      tool,
      args: action.officeOperation.semanticArgs,
      approvalStatus: action.officeOperation.approvalStatus,
      reason: action.officeOperation.reason
    }
  }

  if (action.type === 'skill_installation') {
    return {
      id: action.installation.id,
      tool: 'skills_commit_install',
      args: { installRef: action.installation.installRef },
      approvalStatus: action.installation.approvalStatus,
      reason: 'Install the frozen inspected Skill package.'
    }
  }

  if (action.type !== 'command') return null

  const args: Record<string, unknown> = {
    command: action.command.command
  }
  copyIfDefined(args, 'cwd', action.command.cwd)
  copyIfDefined(args, 'riskLevel', action.command.riskLevel)
  copyIfDefined(args, 'reason', action.command.reason)
  return {
    id: action.command.id,
    tool: 'run_command',
    args,
    approvalStatus: action.command.approvalStatus,
    reason: action.command.reason
  }
}

/**
 * Maps Host action identity to the model Tool-call identity used by the Renderer projection.
 * Most approvals use the same value for both identities. Built-in capability activation keeps
 * them deliberately distinct, so reducers must never compare its actionId directly to Tool-call
 * IDs.
 */
export function getActionToolCallId(action: AgentProposedAction): string | null {
  return getActionToolCall(action)?.id ?? null
}

function copyIfDefined(target: Record<string, unknown>, name: string, value: unknown) {
  if (value !== undefined && value !== null) target[name] = value
}

export function withActionApprovalStatus(
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus
): AgentProposedAction {
  if (action.type === 'command') {
    return { ...action, command: { ...action.command, approvalStatus } }
  }
  if (action.type === 'tool_call') {
    return { ...action, call: { ...action.call, approvalStatus } }
  }
  // Round 5A only freezes the safe MCP contract. Round 5B owns the dedicated approval UI.
  // Updating this already-redacted nested status keeps the temporary reject flow from appearing
  // perpetually pending without reinterpreting the MCP action as a generic Tool call.
  if (action.type === 'mcp_tool_call') {
    return {
      ...action,
      approval: {
        ...action.approval,
        call: { ...action.approval.call, approvalStatus }
      }
    }
  }
  if (action.type === 'builtin_capability_activation') {
    return {
      ...action,
      approval: { ...action.approval, approvalStatus }
    }
  }
  if (action.type === 'file_write') {
    return { ...action, fileWrite: { ...action.fileWrite, approvalStatus } }
  }
  if (action.type === 'skill_materialization') {
    return { ...action, materialization: { ...action.materialization, approvalStatus } }
  }
  if (action.type === 'skill_script') {
    return { ...action, script: { ...action.script, approvalStatus } }
  }
  if (action.type === 'office_operation') {
    return { ...action, officeOperation: { ...action.officeOperation, approvalStatus } }
  }
  if (action.type === 'skill_installation') {
    return { ...action, installation: { ...action.installation, approvalStatus } }
  }
  return { ...action, diff: { ...action.diff, approvalStatus } }
}
