import type {
  AgentApprovalStatus,
  AgentProposedAction,
  AgentToolCall,
  OfficeExecutionRequest,
  OfficeOperationParameters
} from '@mycopilot/protocol'

export function getActionToolCall(action: AgentProposedAction): AgentToolCall | null {
  if (action.type === 'tool_call') return action.call

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
    return {
      id: action.materialization.id,
      tool: 'skills_materialize_resource',
      args: {
        sourceUri: action.materialization.sourceUri,
        sourcePrefix: action.materialization.sourcePrefix,
        destination: action.materialization.destination,
        reason: action.materialization.reason
      },
      approvalStatus: action.materialization.approvalStatus,
      reason: action.materialization.reason
    }
  }

  if (action.type === 'skill_script') {
    return {
      id: action.script.id,
      tool: 'skills_run_script',
      args: {
        scriptUri: action.script.scriptUri,
        interpreter: action.script.interpreter,
        args: action.script.args,
        requirements: action.script.requirements,
        timeoutMs: action.script.timeoutMs,
        reason: action.script.reason
      },
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
      args: getOfficeOperationModelArgs(request, action.officeOperation.reason),
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

  return {
    id: action.command.id,
    tool: 'run_command',
    args: {
      command: action.command.command,
      cwd: action.command.cwd,
      riskLevel: action.command.riskLevel,
      reason: action.command.reason
    },
    approvalStatus: action.command.approvalStatus,
    reason: action.command.reason
  }
}

/**
 * Reconstructs the model-facing Office call from the immutable request without exposing
 * host-owned execution authority such as argv, normalized paths, or provider identity.
 */
function getOfficeOperationModelArgs(
  request: OfficeExecutionRequest,
  reason: string
): Record<string, unknown> {
  const modelRequest: Record<string, unknown> = {
    operation: request.operation
  }

  copyIfDefined(modelRequest, 'filePath', request.documentPath)
  if ('parameters' in request && request.parameters) {
    projectOfficeOperationParameters(modelRequest, request.parameters)
  } else {
    modelRequest.legacyRequest = true
  }
  copyIfDefined(modelRequest, 'outputPath', request.outputPath)
  copyIfDefined(modelRequest, 'destinationPath', request.destinationPath)
  copyIfDefined(modelRequest, 'timeoutMs', request.timeoutMs)

  return { request: modelRequest, reason }
}

function projectOfficeOperationParameters(
  args: Record<string, unknown>,
  parameters: OfficeOperationParameters
) {
  switch (parameters.type) {
    case 'help':
      copyIfDefined(args, 'verb', parameters.verb)
      copyIfDefined(args, 'element', parameters.element)
      return
    case 'create':
      copyIfDefined(args, 'locale', parameters.locale)
      copyIfTrue(args, 'minimal', parameters.minimal)
      copyIfTrue(args, 'overwriteExisting', parameters.overwrite)
      return
    case 'view':
      args.mode = parameters.mode
      copyIfDefined(args, 'start', parameters.start)
      copyIfDefined(args, 'end', parameters.end)
      copyIfDefined(args, 'maxLines', parameters.maxLines)
      copyIfDefined(args, 'issueType', parameters.issueType)
      copyIfDefined(args, 'limit', parameters.limit)
      copyIfNonEmpty(args, 'columns', parameters.columns)
      copyIfNonEmpty(args, 'pages', parameters.pages)
      copyIfDefined(args, 'range', parameters.range)
      copyIfDefined(args, 'viewport', parameters.viewport)
      copyIfDefined(args, 'grid', parameters.grid)
      copyIfDefined(args, 'renderMode', parameters.renderMode)
      copyIfTrue(args, 'includePageCount', parameters.pageCount)
      return
    case 'get':
      copyIfDefined(args, 'target', parameters.target)
      copyIfDefined(args, 'depth', parameters.depth)
      return
    case 'query':
      args.selector = parameters.selector
      copyIfDefined(args, 'containsText', parameters.contains)
      copyIfTrue(args, 'compact', parameters.compact)
      copyIfNonEmpty(args, 'fields', parameters.fields)
      return
    case 'validate':
      return
    case 'set':
      args.target = parameters.target
      copyIfNonEmptyRecord(args, 'properties', parameters.properties)
      copyIfDefined(args, 'textReplacement', parameters.replacement)
      copyIfTrue(args, 'overrideProtection', parameters.force)
      return
    case 'add':
      args.parent = parameters.parent
      args.element = parameters.elementType
      copyIfDefined(args, 'copyFrom', parameters.copyFrom)
      copyIfDefined(args, 'placement', parameters.position)
      copyIfNonEmptyRecord(args, 'properties', parameters.properties)
      copyIfTrue(args, 'overrideProtection', parameters.force)
      return
    case 'remove':
      args.target = parameters.target
      copyIfDefined(args, 'shift', parameters.shift)
      copyIfNonEmptyRecord(args, 'properties', parameters.properties)
      return
    case 'move':
      args.target = parameters.target
      copyIfDefined(args, 'toParent', parameters.newParent)
      copyIfDefined(args, 'placement', parameters.position)
      copyIfNonEmptyRecord(args, 'properties', parameters.properties)
      return
    case 'swap':
      args.firstTarget = parameters.firstTarget
      args.secondTarget = parameters.secondTarget
  }
}

function copyIfDefined(target: Record<string, unknown>, name: string, value: unknown) {
  if (value !== undefined && value !== null) target[name] = value
}

function copyIfTrue(target: Record<string, unknown>, name: string, value: boolean | undefined) {
  if (value === true) target[name] = true
}

function copyIfNonEmpty(
  target: Record<string, unknown>,
  name: string,
  value: unknown[] | undefined
) {
  if (value && value.length > 0) target[name] = value
}

function copyIfNonEmptyRecord(
  target: Record<string, unknown>,
  name: string,
  value: Record<string, unknown> | undefined
) {
  if (value && Object.keys(value).length > 0) target[name] = value
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
