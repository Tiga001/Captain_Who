import type { AgentProposedAction } from '@mycopilot/protocol'

export function getAgentActionApprovalStatus(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.approvalStatus
  if (action.type === 'file_write') return action.fileWrite.approvalStatus
  if (action.type === 'command') return action.command.approvalStatus
  if (action.type === 'skill_materialization') return action.materialization.approvalStatus
  if (action.type === 'skill_script') return action.script.approvalStatus
  if (action.type === 'office_operation') return action.officeOperation.approvalStatus
  if (action.type === 'mcp_tool_call') return action.approval.call.approvalStatus
  return action.call.approvalStatus
}

export function getAgentActionId(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.id
  if (action.type === 'file_write') return action.fileWrite.id
  if (action.type === 'command') return action.command.id
  if (action.type === 'skill_materialization') return action.materialization.id
  if (action.type === 'skill_script') return action.script.id
  if (action.type === 'office_operation') return action.officeOperation.id
  if (action.type === 'mcp_tool_call') return action.approval.identity.actionId
  return action.call.id
}
