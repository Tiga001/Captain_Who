/**
 * Compatibility façade for the Renderer-facing Agent/MCP parsers.
 *
 * Keep this module as the stable legacy import path. The implementation modules
 * below are intentionally private to `@mycopilot/protocol`.
 */
export { parseAgentEventForHost } from './agentParsers/events'
export {
  parseAgentBrowserRiskApproval,
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationApproval,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentBuiltinMcpToolApproval,
  parseAgentBuiltinMcpToolApprovalProposedAction
} from './agentParsers/builtinApprovals'
export {
  parseAgentFileChangeContentPageForHost,
  parseAgentFileChangeDiffPageForHost,
  parseAgentFileChangeHistoryDiffPageForHost,
  parseAgentFileChangeResultForHost
} from './agentParsers/fileChanges'
export {
  parseAgentMcpToolApproval,
  parseAgentToolIdentityForHost
} from './agentParsers/mcpIdentity'
export { parseAgentMcpToolInvocationEvent } from './agentParsers/mcpInvocation'
export { parsePendingAgentActionSnapshotsForHost } from './agentParsers/pendingActions'
export { parseAgentActionExecutionOutputForHost } from './agentParsers/executionOutputs'
export { parseAgentMcpProposedAction } from './agentParsers/proposedActions'
