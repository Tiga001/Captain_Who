import type { AgentChatOutput } from '../agent'
import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  invalidProtocolValue
} from '../skills/validation'
import { parseAgentEventForHost } from './events'
import { parseStrictProposedActions } from './proposedActions'
import {
  MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES,
  expectBoundedString,
  expectOpaqueRunId
} from './shared'
export function parseMcpAgentChatOutput(value: unknown, context: string): AgentChatOutput {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'content',
      'status',
      'runId',
      'events',
      'toolDefinitions',
      'todo',
      'usage',
      'finishReason',
      'proposedActions',
      'conversationTurnTrace'
    ] as const,
    context
  )
  if (!Array.isArray(record.events) || record.events.length > 4096) {
    throw invalidProtocolValue(context, 'events must be an array with at most 4096 items')
  }
  const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
  const events = record.events.flatMap((event) => {
    const eventRecord = expectRecord(event, `${context}.events[]`)
    const isProtectedApproval =
      eventRecord.type === 'approval_required' &&
      typeof eventRecord.action === 'object' &&
      eventRecord.action !== null &&
      !Array.isArray(eventRecord.action) &&
      'type' in eventRecord.action &&
      (eventRecord.action.type === 'mcp_tool_call' ||
        eventRecord.action.type === 'builtin_capability_activation' ||
        eventRecord.action.type === 'builtin_mcp_tool_approval' ||
        eventRecord.action.type === 'browser_risk_approval')
    const isProtectedDone =
      eventRecord.type === 'done' &&
      Array.isArray(eventRecord.proposedActions) &&
      eventRecord.proposedActions.some(
        (action) =>
          typeof action === 'object' &&
          action !== null &&
          !Array.isArray(action) &&
          'type' in action &&
          (action.type === 'mcp_tool_call' ||
            action.type === 'builtin_capability_activation' ||
            action.type === 'builtin_mcp_tool_approval' ||
            action.type === 'browser_risk_approval')
      )
    if (
      eventRecord.type !== 'mcp_tool_invocation_state_changed' &&
      !isProtectedApproval &&
      !isProtectedDone
    ) {
      return []
    }
    const parsedEvent = parseAgentEventForHost(eventRecord)
    if (parsedEvent.runId !== runId) {
      throw invalidProtocolValue(context, 'nested MCP event runId must match output runId')
    }
    return [parsedEvent]
  })
  const proposedActions = parseStrictProposedActions(
    record.proposedActions,
    `${context}.proposedActions`,
    runId
  )
  return {
    content: expectBoundedString(
      record.content,
      `${context}.content`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    status: expectEnum(
      record.status,
      [
        'idle',
        'running',
        'waiting_for_approval',
        'waiting_for_user_input',
        'completed',
        'failed',
        'cancelled'
      ] as const,
      `${context}.status`
    ),
    runId,
    events,
    // Tool definitions can contain Server-authored descriptions and schemas. Round 5B consumes
    // the separate safe Catalog projection instead.
    toolDefinitions: [],
    proposedActions
  }
}
