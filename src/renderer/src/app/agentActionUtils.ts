import type { AgentProposedAction } from '@mycopilot/protocol'

export function getAgentActionApprovalStatus(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.approvalStatus
  if (action.type === 'file_write') return action.fileWrite.approvalStatus
  if (action.type === 'command') return action.command.approvalStatus
  return action.call.approvalStatus
}

export function getAgentActionId(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.id
  if (action.type === 'file_write') return action.fileWrite.id
  if (action.type === 'command') return action.command.id
  return action.call.id
}
