import type { AgentProposedAction, PendingAgentActionSnapshot } from '@mycopilot/protocol'

/** Defense in depth for Renderer hydration if a future Host bridge bypasses strict wire parsing. */
export function shouldHydratePendingAgentAction(snapshot: PendingAgentActionSnapshot): boolean {
  if (snapshot.action.type === 'builtin_capability_activation') {
    const approval = snapshot.action.approval
    return (
      snapshot.status === 'pending' &&
      approval.approvalStatus === 'required' &&
      snapshot.actionId === approval.actionId &&
      snapshot.runId === approval.runId &&
      snapshot.toolName === 'activate_capability' &&
      snapshot.toolCallId === approval.callId
    )
  }
  if (snapshot.action.type === 'browser_risk_approval') {
    const approval = snapshot.action.approval
    return (
      snapshot.status === 'pending' &&
      approval.approvalStatus === 'required' &&
      snapshot.actionId === approval.actionId &&
      snapshot.runId === approval.runId &&
      snapshot.toolName === approval.triggerToolName &&
      snapshot.toolCallId === approval.callId
    )
  }
  if (snapshot.action.type === 'builtin_mcp_tool_approval') {
    const approval = snapshot.action.approval
    return (
      snapshot.status === 'pending' &&
      approval.approvalStatus === 'required' &&
      snapshot.actionId === approval.identity.actionId &&
      snapshot.runId === approval.identity.runId &&
      snapshot.toolName === approval.identity.rawName &&
      snapshot.toolCallId === approval.identity.callId
    )
  }
  return true
}

export function getAgentActionApprovalStatus(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.approvalStatus
  if (action.type === 'file_write') return action.fileWrite.approvalStatus
  if (action.type === 'command') return action.command.approvalStatus
  if (action.type === 'skill_materialization') return action.materialization.approvalStatus
  if (action.type === 'skill_script') return action.script.approvalStatus
  if (action.type === 'office_operation') return action.officeOperation.approvalStatus
  if (action.type === 'skill_installation') return action.installation.approvalStatus
  if (action.type === 'mcp_tool_call') return action.approval.call.approvalStatus
  if (action.type === 'builtin_capability_activation') return action.approval.approvalStatus
  if (action.type === 'browser_risk_approval') return action.approval.approvalStatus
  if (action.type === 'builtin_mcp_tool_approval') return action.approval.approvalStatus
  return action.call.approvalStatus
}

export function getAgentActionId(action: AgentProposedAction) {
  if (action.type === 'diff') return action.diff.id
  if (action.type === 'file_write') return action.fileWrite.id
  if (action.type === 'command') return action.command.id
  if (action.type === 'skill_materialization') return action.materialization.id
  if (action.type === 'skill_script') return action.script.id
  if (action.type === 'office_operation') return action.officeOperation.id
  if (action.type === 'skill_installation') return action.installation.id
  if (action.type === 'mcp_tool_call') return action.approval.identity.actionId
  if (action.type === 'builtin_capability_activation') return action.approval.actionId
  if (action.type === 'browser_risk_approval') return action.approval.actionId
  if (action.type === 'builtin_mcp_tool_approval') return action.approval.identity.actionId
  return action.call.id
}
