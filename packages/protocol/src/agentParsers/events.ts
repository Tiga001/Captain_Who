import type { AgentEvent } from '../agent'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import { parseAgentCommandSessionEvent } from '../agentCommandSessionParsers'
import {
  parseActivatedSkillSummary,
  parseAgentContextWindowSnapshot,
  parseAgentGuidanceEvent,
  parseAgentLlmRetryEvent,
  parseAgentMessageStreamResetEvent,
  parseAgentStateSnapshot,
  parseAgentTodoState,
  parseAgentToolCallEvent,
  parseAgentToolDefinitions,
  parseAgentToolResultForHost,
  parseAgentUsage
} from './eventPayloads'
import {
  parseAgentFileChangePreview,
  parseAgentFileChangeProposal,
  parseAgentFileChangeSnapshot
} from './fileChanges'
import { parseAgentMcpToolInvocationEvent } from './mcpInvocation'
import {
  parseAgentProposedActionForHost,
  parseAgentProposedActionsForHost
} from './proposedActions'
import {
  MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES,
  MAX_RENDERER_SAFE_AGENT_EVENT_BYTES,
  assertRendererSafeJson,
  expectBoundedNonEmptyString,
  expectBoundedString,
  expectOpaqueRunId
} from './shared'
export const AGENT_EVENT_TYPES = {
  started: true,
  tool_set_changed: true,
  state: true,
  message_delta: true,
  message_stream_started: true,
  message_stream_reset: true,
  message_stream_committed: true,
  llm_retry: true,
  tool_input_progress: true,
  file_change_preview_updated: true,
  file_change_preview_cleared: true,
  message: true,
  guidance_queued: true,
  guidance_applied: true,
  guidance_rejected: true,
  tool_call: true,
  tool_result: true,
  mcp_tool_invocation_state_changed: true,
  todo_updated: true,
  skill_activated: true,
  file_change_updated: true,
  context_window_updated: true,
  context_compaction_started: true,
  context_compaction_finished: true,
  approval_required: true,
  file_change_proposed: true,
  command_started: true,
  command_output: true,
  command_exited: true,
  command_interrupted: true,
  error: true,
  done: true
} as const satisfies Record<AgentEvent['type'], true>

/** Canonical strict parser for every Renderer-facing Agent event variant. */
export function parseAgentEventForHost(value: unknown): AgentEvent {
  const record = expectRecord(value, 'Agent event')
  assertRendererSafeJson(record, 'Agent event', MAX_RENDERER_SAFE_AGENT_EVENT_BYTES)
  const type = expectAgentEventType(record.type, 'Agent event.type')
  const context = `Agent ${type} event`

  switch (type) {
    case 'started':
      expectOnlyKeys(record, ['type', 'runId', 'toolDefinitions'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        toolDefinitions: parseAgentToolDefinitions(
          record.toolDefinitions,
          `${context}.toolDefinitions`
        )
      }
    case 'tool_set_changed':
      expectOnlyKeys(
        record,
        [
          'type',
          'runId',
          'stableRevision',
          'dynamicRevision',
          'effectiveRevision',
          'toolDefinitions'
        ] as const,
        context
      )
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        stableRevision: expectOpaqueRunId(record.stableRevision, `${context}.stableRevision`),
        dynamicRevision: expectOpaqueRunId(record.dynamicRevision, `${context}.dynamicRevision`),
        effectiveRevision: expectOpaqueRunId(
          record.effectiveRevision,
          `${context}.effectiveRevision`
        ),
        toolDefinitions: parseAgentToolDefinitions(
          record.toolDefinitions,
          `${context}.toolDefinitions`
        )
      }
    case 'state':
      expectOnlyKeys(record, ['type', 'runId', 'state'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        state: parseAgentStateSnapshot(record.state, `${context}.state`)
      }
    case 'message_delta':
      expectOnlyKeys(record, ['type', 'runId', 'streamId', 'delta'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        ...(record.streamId === undefined
          ? {}
          : { streamId: expectOpaqueRunId(record.streamId, `${context}.streamId`) }),
        delta: expectBoundedString(
          record.delta,
          `${context}.delta`,
          MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
        )
      }
    case 'message_stream_started':
      expectOnlyKeys(record, ['type', 'runId', 'streamId', 'attempt'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        streamId: expectOpaqueRunId(record.streamId, `${context}.streamId`),
        attempt: expectSafeInteger(record.attempt, `${context}.attempt`, 0)
      }
    case 'message_stream_reset':
      return parseAgentMessageStreamResetEvent(record)
    case 'message_stream_committed':
      expectOnlyKeys(record, ['type', 'runId', 'streamId', 'traceSequence'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        streamId: expectOpaqueRunId(record.streamId, `${context}.streamId`),
        traceSequence:
          record.traceSequence === null
            ? null
            : expectSafeInteger(record.traceSequence, `${context}.traceSequence`, 0)
      }
    case 'llm_retry':
      return parseAgentLlmRetryEvent(record)
    case 'tool_input_progress':
      expectOnlyKeys(
        record,
        [
          'type',
          'runId',
          'streamId',
          'attempt',
          'toolCallIndex',
          'toolCallId',
          'tool',
          'receivedBytes'
        ] as const,
        context
      )
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        streamId: expectOpaqueRunId(record.streamId, `${context}.streamId`),
        attempt: expectSafeInteger(record.attempt, `${context}.attempt`, 0),
        toolCallIndex: expectSafeInteger(record.toolCallIndex, `${context}.toolCallIndex`, 0),
        ...(record.toolCallId === undefined
          ? {}
          : {
              toolCallId: expectBoundedNonEmptyString(
                record.toolCallId,
                `${context}.toolCallId`,
                2048
              )
            }),
        tool: expectBoundedNonEmptyString(record.tool, `${context}.tool`, 1024),
        receivedBytes: expectSafeInteger(record.receivedBytes, `${context}.receivedBytes`, 0)
      }
    case 'file_change_preview_updated':
      expectOnlyKeys(record, ['type', 'runId', 'preview'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        preview: parseAgentFileChangePreview(record.preview, `${context}.preview`)
      }
    case 'file_change_preview_cleared':
      expectOnlyKeys(record, ['type', 'runId', 'streamId', 'attempt'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        streamId: expectOpaqueRunId(record.streamId, `${context}.streamId`),
        attempt: expectSafeInteger(record.attempt, `${context}.attempt`, 0)
      }
    case 'message':
      expectOnlyKeys(record, ['type', 'runId', 'content'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        content: expectBoundedString(
          record.content,
          `${context}.content`,
          MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
        )
      }
    case 'guidance_queued':
    case 'guidance_applied':
    case 'guidance_rejected':
      return parseAgentGuidanceEvent(type, record)
    case 'tool_call':
      return parseAgentToolCallEvent(record)
    case 'tool_result':
      expectOnlyKeys(record, ['type', 'runId', 'result'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        result: parseAgentToolResultForHost(record.result, `${context}.result`)
      }
    case 'mcp_tool_invocation_state_changed':
      expectOnlyKeys(record, ['type', 'runId', 'invocation'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        invocation: parseAgentMcpToolInvocationEvent(record.invocation)
      }
    case 'todo_updated':
      expectOnlyKeys(record, ['type', 'runId', 'todo'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        todo: parseAgentTodoState(record.todo, `${context}.todo`)
      }
    case 'skill_activated':
      expectOnlyKeys(
        record,
        ['type', 'runId', 'activationRevision', 'activatedBy', 'skill'] as const,
        context
      )
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        activationRevision: expectOpaqueRunId(
          record.activationRevision,
          `${context}.activationRevision`
        ),
        activatedBy: expectEnum(
          record.activatedBy,
          ['user', 'model'] as const,
          `${context}.activatedBy`
        ),
        skill: parseActivatedSkillSummary(record.skill, `${context}.skill`)
      }
    case 'file_change_updated':
      expectOnlyKeys(record, ['type', 'runId', 'fileChange'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        fileChange: parseAgentFileChangeSnapshot(record.fileChange, `${context}.fileChange`)
      }
    case 'context_window_updated':
      expectOnlyKeys(record, ['type', 'runId', 'conversationId', 'snapshot'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        ...(record.conversationId === undefined
          ? {}
          : {
              conversationId: expectOpaqueRunId(record.conversationId, `${context}.conversationId`)
            }),
        snapshot: parseAgentContextWindowSnapshot(record.snapshot, `${context}.snapshot`)
      }
    case 'context_compaction_started':
      expectOnlyKeys(record, ['type', 'runId', 'operationId', 'traceSequence'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        operationId: expectOpaqueRunId(record.operationId, `${context}.operationId`),
        traceSequence: expectSafeInteger(record.traceSequence, `${context}.traceSequence`, 0)
      }
    case 'context_compaction_finished':
      expectOnlyKeys(
        record,
        ['type', 'runId', 'operationId', 'outcome', 'traceSequence'] as const,
        context
      )
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        operationId: expectOpaqueRunId(record.operationId, `${context}.operationId`),
        outcome: expectEnum(
          record.outcome,
          ['applied', 'skipped', 'failed', 'cancelled'] as const,
          `${context}.outcome`
        ),
        traceSequence: expectSafeInteger(record.traceSequence, `${context}.traceSequence`, 0)
      }
    case 'approval_required': {
      expectOnlyKeys(record, ['type', 'runId', 'action'] as const, context)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      return {
        type,
        runId,
        action: parseAgentProposedActionForHost(record.action, `${context}.action`, runId, true)
      }
    }
    case 'file_change_proposed':
      expectOnlyKeys(record, ['type', 'runId', 'fileChange'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        fileChange: parseAgentFileChangeProposal(record.fileChange, `${context}.fileChange`)
      }
    case 'command_started':
    case 'command_output':
    case 'command_exited':
    case 'command_interrupted':
      return parseAgentCommandSessionEvent(record)
    case 'error': {
      expectOnlyKeys(
        record,
        ['type', 'runId', 'traceSequence', 'message', 'recoverable', 'code', 'details'] as const,
        context
      )
      const runId =
        record.runId === null || record.runId === undefined
          ? undefined
          : expectOpaqueRunId(record.runId, `${context}.runId`)
      const code =
        record.code === undefined
          ? undefined
          : expectBoundedNonEmptyString(record.code, `${context}.code`, 128)
      if (record.details !== undefined) {
        assertRendererSafeJson(record.details, `${context}.details`)
      }
      return {
        type,
        ...(runId === undefined ? {} : { runId }),
        traceSequence:
          record.traceSequence === null
            ? null
            : expectSafeInteger(record.traceSequence, `${context}.traceSequence`, 0),
        message: expectBoundedString(
          record.message,
          `${context}.message`,
          MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
        ),
        recoverable: expectBoolean(record.recoverable, `${context}.recoverable`),
        ...(code === undefined ? {} : { code }),
        ...(record.details === undefined ? {} : { details: record.details })
      }
    }
    case 'done': {
      expectOnlyKeys(
        record,
        [
          'type',
          'runId',
          'success',
          'status',
          'content',
          'usage',
          'finishReason',
          'proposedActions'
        ] as const,
        context
      )
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      return {
        type,
        runId,
        success: expectBoolean(record.success, `${context}.success`),
        ...(record.status === undefined
          ? {}
          : {
              status: expectEnum(
                record.status,
                [
                  'idle',
                  'running',
                  'waiting_for_approval',
                  'completed',
                  'failed',
                  'cancelled'
                ] as const,
                `${context}.status`
              )
            }),
        ...(record.content === undefined
          ? {}
          : {
              content: expectBoundedString(
                record.content,
                `${context}.content`,
                MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
              )
            }),
        ...(record.usage === undefined
          ? {}
          : { usage: parseAgentUsage(record.usage, `${context}.usage`) }),
        ...(record.finishReason === undefined
          ? {}
          : {
              finishReason: expectBoundedString(
                record.finishReason,
                `${context}.finishReason`,
                2048
              )
            }),
        ...(record.proposedActions === undefined
          ? {}
          : {
              proposedActions: parseAgentProposedActionsForHost(
                record.proposedActions,
                `${context}.proposedActions`,
                runId
              )
            })
      }
    }
    default:
      return assertNeverAgentEventType(type)
  }
}

export function expectAgentEventType(value: unknown, context: string): AgentEvent['type'] {
  const type = expectBoundedNonEmptyString(value, context, 128)
  if (!Object.hasOwn(AGENT_EVENT_TYPES, type)) {
    throw invalidProtocolValue(context, `unknown event type ${type}`)
  }
  return type as AgentEvent['type']
}

export function assertNeverAgentEventType(value: never): never {
  throw invalidProtocolValue('Agent event.type', `unknown event type ${String(value)}`)
}
