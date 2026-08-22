import type {
  AgentActionExecutionOutput,
  AgentBrowserAddressClass,
  AgentBrowserRiskApproval,
  AgentBrowserRiskKind,
  AgentBrowserReviewedToolName,
  AgentBrowserRiskTrigger,
  AgentBuiltinCapabilityActivationApproval,
  AgentBuiltinMcpToolApproval,
  AgentBuiltinMcpToolRiskKind,
  AgentChatOutput,
  AgentCommandActionProjection,
  AgentContextWindowSnapshot,
  AgentDiffProposal,
  AgentEvent,
  AgentFileDraftSnapshot,
  AgentFileWritePreview,
  AgentFileWriteProposal,
  AgentLlmRetryCategory,
  AgentMcpArgumentSummary,
  AgentMcpInvocationDiagnostics,
  AgentMcpServerScope,
  AgentMcpToolApproval,
  AgentMcpToolApprovalSummary,
  AgentMcpToolInvocationEvent,
  AgentMcpToolInvocationIdentity,
  AgentMcpToolProvenance,
  AgentOfficeOperationRequest,
  AgentProposedAction,
  AgentSkillInstallationRequest,
  AgentSkillMaterializationRequest,
  AgentSkillScriptRequest,
  AgentStateSnapshot,
  AgentTodoState,
  AgentToolDefinition,
  AgentToolIdentity,
  AgentToolCall,
  AgentToolResult,
  AgentUsage,
  ConversationTraceAttachment,
  PendingAgentActionSnapshot
} from './agent'
import type { ActivatedSkillSummary } from './skills'
import { parseMcpBuiltinCapabilityId } from './mcp/parsers'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'
import { parseAgentCommandSessionEvent } from './agentCommandSessionParsers'

const DIGEST_PATTERN = /^[0-9a-f]{64}$/
const UUID_V4_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const SAFE_CODE_PATTERN = /^[a-zA-Z0-9_.-]{1,128}$/
const CANONICAL_PROVIDER_CODE_PATTERN = /^[a-z0-9][a-z0-9_.-]{0,127}$/
const MODEL_TOOL_CALL_ID_PATTERN = /^tc1_[a-zA-Z0-9_-]{43}$/
const MCP_APPROVAL_TTL_MS = 15 * 60 * 1000
const BUILTIN_CAPABILITY_APPROVAL_TTL_SECONDS = 15 * 60
const BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS = 15 * 60
const BROWSER_RISK_APPROVAL_TTL_SECONDS = 15 * 60
const MAX_RENDERER_DATE_UNIX_SECONDS = 253_402_300_799
const MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES = 1024 * 1024
const MAX_RENDERER_SAFE_AGENT_EVENT_BYTES = 16 * 1024 * 1024
const MAX_RENDERER_SAFE_PROPOSED_ACTIONS = 1024
const MAX_MCP_DIAGNOSTIC_ARGUMENT_BYTES = 64 * 1024
const MAX_MCP_DIAGNOSTIC_ARGUMENT_VALUES = 4096
const MAX_MCP_DIAGNOSTIC_ARGUMENT_DEPTH = 32
const MAX_MCP_DIAGNOSTIC_RESULT_BLOCKS = 128
const MAX_MCP_DIAGNOSTIC_RESULT_BYTES = 4 * 1024 * 1024
const MAX_LLM_RETRY_DELAY_MS = 60_000
const MAX_LLM_RETRY_ATTEMPTS = 6
const LLM_RETRY_CATEGORIES = [
  'rate_limited',
  'quota_exhausted',
  'overloaded',
  'authentication',
  'invalid_request',
  'context_too_large',
  'network',
  'unknown'
] as const satisfies readonly AgentLlmRetryCategory[]
const BROWSER_ADDRESS_CLASSES = [
  'public',
  'loopback',
  'private',
  'link_local',
  'cloud_metadata',
  'unresolved'
] as const satisfies readonly AgentBrowserAddressClass[]
const BROWSER_RISK_KINDS = [
  'insecure_http',
  'localhost',
  'loopback',
  'private_network',
  'link_local',
  'cloud_metadata',
  'non_standard_port',
  'url_userinfo',
  'dns_private_resolution',
  'risk_escalation',
  'new_window',
  'file_upload',
  'file_download',
  'local_service_request'
] as const satisfies readonly AgentBrowserRiskKind[]
const BROWSER_RISK_TRIGGERS = [
  'tool_argument',
  'main_frame',
  'redirect',
  'new_window',
  'subresource',
  'upload',
  'download'
] as const satisfies readonly AgentBrowserRiskTrigger[]
const BROWSER_REVIEWED_TOOL_NAMES = [
  'browser_navigate',
  'browser_snapshot',
  'browser_find',
  'browser_click',
  'browser_type',
  'browser_fill_form',
  'browser_press_key',
  'browser_tabs',
  'browser_wait_for',
  'browser_close'
] as const satisfies readonly AgentBrowserReviewedToolName[]
const BUILTIN_MCP_TOOL_RISK_KINDS = [
  'file_read',
  'file_write',
  'file_upload',
  'file_download',
  'cookie_read',
  'cookie_write',
  'local_storage_read',
  'local_storage_write',
  'session_storage_read',
  'session_storage_write',
  'storage_state_import',
  'storage_state_export',
  'network_sensitive_read',
  'page_script_execution',
  'unsafe_code_execution'
] as const satisfies readonly AgentBuiltinMcpToolRiskKind[]
const BUILTIN_MCP_APPROVAL_TOOL_NAMES = [
  'browser_cookie_clear',
  'browser_cookie_delete',
  'browser_cookie_get',
  'browser_cookie_list',
  'browser_cookie_set',
  'browser_drop',
  'browser_evaluate',
  'browser_file_upload',
  'browser_localstorage_clear',
  'browser_localstorage_delete',
  'browser_localstorage_get',
  'browser_localstorage_list',
  'browser_localstorage_set',
  'browser_network_request',
  'browser_sessionstorage_clear',
  'browser_sessionstorage_delete',
  'browser_sessionstorage_get',
  'browser_sessionstorage_list',
  'browser_sessionstorage_set',
  'browser_set_storage_state',
  'browser_storage_state'
] as const

const AGENT_EVENT_TYPES = {
  started: true,
  tool_set_changed: true,
  state: true,
  message_delta: true,
  message_stream_started: true,
  message_stream_reset: true,
  message_stream_committed: true,
  llm_retry: true,
  tool_input_progress: true,
  file_write_preview_updated: true,
  file_write_preview_cleared: true,
  message: true,
  guidance_queued: true,
  guidance_applied: true,
  guidance_rejected: true,
  tool_call: true,
  tool_result: true,
  mcp_tool_invocation_state_changed: true,
  todo_updated: true,
  skill_activated: true,
  file_draft_updated: true,
  context_window_updated: true,
  context_compaction_started: true,
  context_compaction_finished: true,
  approval_required: true,
  diff: true,
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
    case 'file_write_preview_updated':
      expectOnlyKeys(record, ['type', 'runId', 'preview'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        preview: parseAgentFileWritePreview(record.preview, `${context}.preview`)
      }
    case 'file_write_preview_cleared':
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
    case 'file_draft_updated':
      expectOnlyKeys(record, ['type', 'runId', 'draft'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        draft: parseAgentFileDraftSnapshot(record.draft, `${context}.draft`)
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
    case 'diff':
      expectOnlyKeys(record, ['type', 'runId', 'diff'] as const, context)
      return {
        type,
        runId: expectOpaqueRunId(record.runId, `${context}.runId`),
        diff: parseAgentDiffProposal(record.diff, `${context}.diff`)
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

function expectAgentEventType(value: unknown, context: string): AgentEvent['type'] {
  const type = expectBoundedNonEmptyString(value, context, 128)
  if (!Object.hasOwn(AGENT_EVENT_TYPES, type)) {
    throw invalidProtocolValue(context, `unknown event type ${type}`)
  }
  return type as AgentEvent['type']
}

function assertNeverAgentEventType(value: never): never {
  throw invalidProtocolValue('Agent event.type', `unknown event type ${String(value)}`)
}

function assertRendererSafeJson(
  value: unknown,
  context: string,
  maximumBytes = MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
): void {
  let encoded: string | undefined
  try {
    encoded = JSON.stringify(value)
  } catch {
    throw invalidProtocolValue(context, 'must be JSON encodable')
  }
  if (encoded === undefined || new TextEncoder().encode(encoded).byteLength > maximumBytes) {
    throw invalidProtocolValue(context, `exceeded the Renderer-safe ${maximumBytes} byte limit`)
  }
}

function expectBoundedArray(value: unknown, context: string, maximumItems: number): unknown[] {
  if (!Array.isArray(value) || value.length > maximumItems) {
    throw invalidProtocolValue(context, `expected an array with at most ${maximumItems} items`)
  }
  return value
}

function parseAgentToolDefinitions(value: unknown, context: string): AgentToolDefinition[] {
  return expectBoundedArray(value, context, 4096).map((entry, index) => {
    const itemContext = `${context}[${index}]`
    const item = expectRecord(entry, itemContext)
    expectOnlyKeys(
      item,
      [
        'name',
        'description',
        'inputSchema',
        'safety',
        'requiresWorkspace',
        'requiresApproval',
        'approvalMode'
      ] as const,
      itemContext
    )
    assertRendererSafeJson(item.inputSchema, `${itemContext}.inputSchema`, 4 * 1024 * 1024)
    return {
      name: expectBoundedNonEmptyString(item.name, `${itemContext}.name`, 1024),
      description: expectBoundedString(item.description, `${itemContext}.description`, 64 * 1024),
      inputSchema: item.inputSchema,
      safety: expectEnum(
        item.safety,
        ['read_only', 'requires_approval', 'destructive'] as const,
        `${itemContext}.safety`
      ),
      requiresWorkspace: expectBoolean(item.requiresWorkspace, `${itemContext}.requiresWorkspace`),
      requiresApproval: expectBoolean(item.requiresApproval, `${itemContext}.requiresApproval`),
      approvalMode: expectEnum(
        item.approvalMode,
        ['never', 'always', 'dynamic'] as const,
        `${itemContext}.approvalMode`
      )
    }
  })
}

function parseAgentStateSnapshot(value: unknown, context: string): AgentStateSnapshot {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['status', 'activeRunId', 'lastError', 'updatedAt'] as const, context)
  return {
    status: expectEnum(
      item.status,
      ['idle', 'running', 'waiting_for_approval', 'completed', 'failed', 'cancelled'] as const,
      `${context}.status`
    ),
    activeRunId:
      item.activeRunId === null
        ? null
        : expectOpaqueRunId(item.activeRunId, `${context}.activeRunId`),
    lastError:
      item.lastError === null
        ? null
        : expectBoundedString(
            item.lastError,
            `${context}.lastError`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          ),
    updatedAt: expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  }
}

function parseAgentFileWritePreview(value: unknown, context: string): AgentFileWritePreview {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'previewId',
      'streamId',
      'attempt',
      'toolCallIndex',
      'toolCallId',
      'draftId',
      'filePath',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'generatedBytes',
      'contentOffsetBytes',
      'contentDelta',
      'updatedAt'
    ] as const,
    context
  )
  return {
    previewId: expectOpaqueRunId(item.previewId, `${context}.previewId`),
    streamId: expectOpaqueRunId(item.streamId, `${context}.streamId`),
    attempt: expectSafeInteger(item.attempt, `${context}.attempt`, 0),
    toolCallIndex: expectSafeInteger(item.toolCallIndex, `${context}.toolCallIndex`, 0),
    ...(item.toolCallId === undefined
      ? {}
      : {
          toolCallId: expectBoundedNonEmptyString(item.toolCallId, `${context}.toolCallId`, 2048)
        }),
    draftId: expectOpaqueRunId(item.draftId, `${context}.draftId`),
    filePath: expectBoundedString(item.filePath, `${context}.filePath`, 16 * 1024),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount: expectSafeInteger(item.lineCount, `${context}.lineCount`, 0),
    byteCount: expectSafeInteger(item.byteCount, `${context}.byteCount`, 0),
    generatedBytes: expectSafeInteger(item.generatedBytes, `${context}.generatedBytes`, 0),
    contentOffsetBytes: expectSafeInteger(
      item.contentOffsetBytes,
      `${context}.contentOffsetBytes`,
      0
    ),
    contentDelta: expectBoundedString(
      item.contentDelta,
      `${context}.contentDelta`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    updatedAt: expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  }
}

function parseConversationTraceAttachments(
  value: unknown,
  context: string
): ConversationTraceAttachment[] {
  return expectBoundedArray(value, context, 256).map((entry, index) => {
    const itemContext = `${context}[${index}]`
    const item = expectRecord(entry, itemContext)
    expectOnlyKeys(item, ['id', 'kind', 'name', 'mimeType', 'sizeBytes'] as const, itemContext)
    return {
      id: expectOpaqueRunId(item.id, `${itemContext}.id`),
      kind: expectEnum(item.kind, ['file', 'image'] as const, `${itemContext}.kind`),
      name: expectBoundedString(item.name, `${itemContext}.name`, 16 * 1024),
      ...(item.mimeType === undefined
        ? {}
        : {
            mimeType: expectBoundedNonEmptyString(item.mimeType, `${itemContext}.mimeType`, 1024)
          }),
      sizeBytes: expectSafeInteger(item.sizeBytes, `${itemContext}.sizeBytes`, 0)
    }
  })
}

function parseAgentGuidanceEvent(
  type: 'guidance_queued' | 'guidance_applied' | 'guidance_rejected',
  record: Record<string, unknown>
): Extract<AgentEvent, { type: typeof type }> {
  const context = `Agent ${type} event`
  const commonKeys = [
    'type',
    'runId',
    'guidanceId',
    'clientMessageId',
    'content',
    'attachments',
    'createdAt'
  ] as const
  if (type === 'guidance_queued') {
    expectOnlyKeys(record, commonKeys, context)
    return {
      type,
      runId: expectOpaqueRunId(record.runId, `${context}.runId`),
      guidanceId: expectOpaqueRunId(record.guidanceId, `${context}.guidanceId`),
      clientMessageId: expectOpaqueRunId(record.clientMessageId, `${context}.clientMessageId`),
      content: expectBoundedString(
        record.content,
        `${context}.content`,
        MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
      ),
      attachments: parseConversationTraceAttachments(record.attachments, `${context}.attachments`),
      createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
    }
  }
  if (type === 'guidance_applied') {
    expectOnlyKeys(record, [...commonKeys, 'sequence'] as const, context)
    return {
      type,
      runId: expectOpaqueRunId(record.runId, `${context}.runId`),
      guidanceId: expectOpaqueRunId(record.guidanceId, `${context}.guidanceId`),
      clientMessageId: expectOpaqueRunId(record.clientMessageId, `${context}.clientMessageId`),
      content: expectBoundedString(
        record.content,
        `${context}.content`,
        MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
      ),
      attachments: parseConversationTraceAttachments(record.attachments, `${context}.attachments`),
      createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
      sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 0)
    }
  }
  expectOnlyKeys(
    record,
    [
      'type',
      'runId',
      'guidanceId',
      'clientMessageId',
      'content',
      'rejectionCode',
      'message',
      'createdAt'
    ] as const,
    context
  )
  return {
    type,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    guidanceId: expectOpaqueRunId(record.guidanceId, `${context}.guidanceId`),
    clientMessageId: expectOpaqueRunId(record.clientMessageId, `${context}.clientMessageId`),
    content: expectBoundedString(
      record.content,
      `${context}.content`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    rejectionCode: expectEnum(
      record.rejectionCode,
      [
        'run_not_steerable',
        'run_interrupted',
        'conversation_mismatch',
        'identity_conflict',
        'attachments_not_supported',
        'model_does_not_support_attachments',
        'attachment_validation_failed',
        'attachment_limit_exceeded',
        'attachment_persistence_failed'
      ] as const,
      `${context}.rejectionCode`
    ),
    message: expectBoundedString(
      record.message,
      `${context}.message`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  }
}

function parseAgentToolCallForHost(value: unknown, context: string): AgentToolCall {
  const call = expectRecord(value, context)
  expectOnlyKeys(call, ['id', 'tool', 'args', 'approvalStatus', 'reason'] as const, context)
  if (!Object.hasOwn(call, 'reason')) {
    throw invalidProtocolValue(context, 'reason is required')
  }
  assertRendererSafeJson(call.args, `${context}.args`, 4 * 1024 * 1024)
  return {
    id: expectModelToolCallId(call.id, `${context}.id`),
    tool: expectBoundedNonEmptyString(call.tool, `${context}.tool`, 256),
    args: call.args,
    approvalStatus: expectEnum(
      call.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    ),
    reason: call.reason === null ? null : expectDisplayText(call.reason, `${context}.reason`, 4096)
  }
}

function parseAgentToolCallEvent(
  record: Record<string, unknown>
): Extract<AgentEvent, { type: 'tool_call' }> {
  const context = 'Agent tool_call event'
  expectOnlyKeys(record, ['type', 'runId', 'traceSequence', 'call', 'identity'] as const, context)
  const call = parseAgentToolCallForHost(record.call, `${context}.call`)
  const identity = parseAgentToolIdentityForHost(record.identity)
  const identityToolName =
    identity.type === 'mcp'
      ? identity.provenance.modelToolName
      : identity.type === 'builtin_capability'
        ? identity.modelName
        : identity.toolName
  if (identityToolName !== call.tool) {
    throw invalidProtocolValue(context, 'identity must match call.tool')
  }
  if (identity.type === 'builtin_capability') {
    // Managed arguments can contain credentials and form values. Renderer needs only activity.
    call.args = {}
  }
  return {
    type: 'tool_call',
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    traceSequence: expectSafeInteger(record.traceSequence, `${context}.traceSequence`, 0),
    call,
    identity
  }
}

function parseAgentToolResultForHost(value: unknown, context: string): AgentToolResult {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['callId', 'tool', 'ok', 'result', 'error'] as const, context)
  if (item.result !== undefined) {
    assertRendererSafeJson(item.result, `${context}.result`, 4 * 1024 * 1024)
  }
  return {
    callId: expectBoundedNonEmptyString(item.callId, `${context}.callId`, 2048),
    tool: expectBoundedNonEmptyString(item.tool, `${context}.tool`, 1024),
    ok: expectBoolean(item.ok, `${context}.ok`),
    ...(item.result === undefined ? {} : { result: item.result }),
    ...(item.error === undefined
      ? {}
      : {
          error: expectBoundedString(
            item.error,
            `${context}.error`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          )
        })
  }
}

function parseAgentTodoState(value: unknown, context: string): AgentTodoState {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['revision', 'items', 'updatedAt'] as const, context)
  const items = expectBoundedArray(item.items, `${context}.items`, 4096).map((entry, index) => {
    const itemContext = `${context}.items[${index}]`
    const todo = expectRecord(entry, itemContext)
    expectOnlyKeys(todo, ['id', 'title', 'status', 'note', 'createdAt', 'updatedAt'], itemContext)
    return {
      id: expectOpaqueRunId(todo.id, `${itemContext}.id`),
      title: expectBoundedString(todo.title, `${itemContext}.title`, 16 * 1024),
      status: expectEnum(
        todo.status,
        ['pending', 'in_progress', 'completed', 'blocked'] as const,
        `${itemContext}.status`
      ),
      ...(todo.note === undefined
        ? {}
        : { note: expectBoundedString(todo.note, `${itemContext}.note`, 64 * 1024) }),
      createdAt: expectSafeInteger(todo.createdAt, `${itemContext}.createdAt`, 0),
      updatedAt: expectSafeInteger(todo.updatedAt, `${itemContext}.updatedAt`, 0)
    }
  })
  return {
    revision: expectSafeInteger(item.revision, `${context}.revision`, 0),
    items,
    updatedAt: expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  }
}

function parseActivatedSkillSummary(value: unknown, context: string): ActivatedSkillSummary {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['id', 'name', 'revision', 'source'] as const, context)
  const source = expectRecord(item.source, `${context}.source`)
  expectOnlyKeys(source, ['kind', 'id'] as const, `${context}.source`)
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    name: expectBoundedString(item.name, `${context}.name`, 1024),
    revision: expectOpaqueRunId(item.revision, `${context}.revision`),
    source: {
      kind: expectEnum(
        source.kind,
        ['workspace', 'bundled', 'installed'] as const,
        `${context}.source.kind`
      ),
      id: expectOpaqueRunId(source.id, `${context}.source.id`)
    }
  }
}

function parseAgentFileDraftSnapshot(value: unknown, context: string): AgentFileDraftSnapshot {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'draftId',
      'conversationId',
      'projectId',
      'filePath',
      'mode',
      'status',
      'baseRevision',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'chunkCount',
      'nextChunkIndex',
      'statsFinal',
      'summary',
      'createdAt',
      'updatedAt'
    ] as const,
    context
  )
  return {
    draftId: expectOpaqueRunId(item.draftId, `${context}.draftId`),
    conversationId: expectOpaqueRunId(item.conversationId, `${context}.conversationId`),
    ...(item.projectId === undefined
      ? {}
      : { projectId: expectOpaqueRunId(item.projectId, `${context}.projectId`) }),
    filePath: expectBoundedString(item.filePath, `${context}.filePath`, 16 * 1024),
    mode: expectEnum(
      item.mode,
      ['create', 'rewrite', 'modify', 'append', 'upsert'] as const,
      `${context}.mode`
    ),
    status: expectEnum(
      item.status,
      [
        'drafting',
        'ready',
        'waiting_approval',
        'applying',
        'applied',
        'rejected',
        'conflict',
        'failed',
        'aborted',
        'expired'
      ] as const,
      `${context}.status`
    ),
    ...(item.baseRevision === undefined
      ? {}
      : {
          baseRevision: expectBoundedNonEmptyString(
            item.baseRevision,
            `${context}.baseRevision`,
            2048
          )
        }),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount: expectSafeInteger(item.lineCount, `${context}.lineCount`, 0),
    byteCount: expectSafeInteger(item.byteCount, `${context}.byteCount`, 0),
    chunkCount: expectSafeInteger(item.chunkCount, `${context}.chunkCount`, 0),
    nextChunkIndex: expectSafeInteger(item.nextChunkIndex, `${context}.nextChunkIndex`, 0),
    statsFinal: expectBoolean(item.statsFinal, `${context}.statsFinal`),
    ...(item.summary === undefined
      ? {}
      : { summary: expectBoundedString(item.summary, `${context}.summary`, 16 * 1024) }),
    createdAt: expectSafeInteger(item.createdAt, `${context}.createdAt`, 0),
    updatedAt: expectSafeInteger(item.updatedAt, `${context}.updatedAt`, 0)
  }
}

function parseAgentContextWindowSnapshot(
  value: unknown,
  context: string
): AgentContextWindowSnapshot {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'model',
      'status',
      'contextWindowTokens',
      'reservedOutputTokens',
      'safetyMarginTokens',
      'inputCapacityTokens',
      'inputTokens',
      'costBreakdown',
      'remainingInputTokens'
    ] as const,
    context
  )
  const cost = expectRecord(item.costBreakdown, `${context}.costBreakdown`)
  expectOnlyKeys(
    cost,
    [
      'systemTokens',
      'toolSchemaTokens',
      'summaryTokens',
      'worldStateTokens',
      'todoTokens',
      'providerContinuationTokens',
      'recentHistoryTokens',
      'totalInputTokens'
    ] as const,
    `${context}.costBreakdown`
  )
  const nonNegative = (field: string): number =>
    expectSafeInteger(cost[field], `${context}.costBreakdown.${field}`, 0)
  return {
    model: expectBoundedNonEmptyString(item.model, `${context}.model`, 1024),
    status: expectEnum(
      item.status,
      ['unconfigured', 'within_budget', 'over_budget', 'invalid_configuration'] as const,
      `${context}.status`
    ),
    ...(item.contextWindowTokens === undefined
      ? {}
      : {
          contextWindowTokens: expectSafeInteger(
            item.contextWindowTokens,
            `${context}.contextWindowTokens`,
            0
          )
        }),
    reservedOutputTokens: expectSafeInteger(
      item.reservedOutputTokens,
      `${context}.reservedOutputTokens`,
      0
    ),
    safetyMarginTokens: expectSafeInteger(
      item.safetyMarginTokens,
      `${context}.safetyMarginTokens`,
      0
    ),
    ...(item.inputCapacityTokens === undefined
      ? {}
      : {
          inputCapacityTokens: expectSafeInteger(
            item.inputCapacityTokens,
            `${context}.inputCapacityTokens`,
            0
          )
        }),
    inputTokens: expectSafeInteger(item.inputTokens, `${context}.inputTokens`, 0),
    costBreakdown: {
      systemTokens: nonNegative('systemTokens'),
      toolSchemaTokens: nonNegative('toolSchemaTokens'),
      summaryTokens: nonNegative('summaryTokens'),
      worldStateTokens: nonNegative('worldStateTokens'),
      todoTokens: nonNegative('todoTokens'),
      providerContinuationTokens: nonNegative('providerContinuationTokens'),
      recentHistoryTokens: nonNegative('recentHistoryTokens'),
      totalInputTokens: nonNegative('totalInputTokens')
    },
    ...(item.remainingInputTokens === undefined
      ? {}
      : {
          remainingInputTokens: expectSignedSafeInteger(
            item.remainingInputTokens,
            `${context}.remainingInputTokens`
          )
        })
  }
}

function expectSignedSafeInteger(value: unknown, context: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) {
    throw invalidProtocolValue(context, 'expected a safe integer')
  }
  return value
}

function parseAgentDiffProposal(value: unknown, context: string): AgentDiffProposal {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['id', 'operation', 'filePath', 'patch', 'baseRevision', 'summary', 'approvalStatus'] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    operation: expectEnum(
      item.operation,
      ['create', 'update', 'delete'] as const,
      `${context}.operation`
    ),
    filePath: expectBoundedString(item.filePath, `${context}.filePath`, 16 * 1024),
    patch: expectBoundedString(
      item.patch,
      `${context}.patch`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    baseRevision:
      item.baseRevision === null
        ? null
        : expectBoundedNonEmptyString(item.baseRevision, `${context}.baseRevision`, 2048),
    summary:
      item.summary === null
        ? null
        : expectBoundedString(item.summary, `${context}.summary`, 16 * 1024),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`)
  }
}

function parseAgentUsage(value: unknown, context: string): AgentUsage {
  const item = expectRecord(value, context)
  const keys = [
    'inputTokens',
    'outputTokens',
    'outputThinkingTokens',
    'totalTokens',
    'cachedInputTokens',
    'cacheCreationInputTokens',
    'billableRequestCount'
  ] as const
  expectOnlyKeys(item, keys, context)
  const usage: AgentUsage = {}
  for (const key of keys) {
    if (item[key] !== undefined) {
      usage[key] = expectSafeInteger(item[key], `${context}.${key}`, 0)
    }
  }
  return usage
}

function parseAgentApprovalStatus(value: unknown, context: string) {
  return expectEnum(value, ['not_required', 'required', 'approved', 'rejected'] as const, context)
}

function parseAgentProposedActionsForHost(
  value: unknown,
  context: string,
  runId: string
): AgentProposedAction[] {
  return expectBoundedArray(value, context, MAX_RENDERER_SAFE_PROPOSED_ACTIONS).map(
    (action, index) => parseAgentProposedActionForHost(action, `${context}[${index}]`, runId, false)
  )
}

function parseAgentProposedActionForHost(
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
    case 'diff':
      expectOnlyKeys(item, ['type', 'diff'] as const, context)
      action = { type, diff: parseAgentDiffProposal(item.diff, `${context}.diff`) }
      break
    case 'file_write':
      expectOnlyKeys(item, ['type', 'fileWrite'] as const, context)
      action = {
        type,
        fileWrite: parseAgentFileWriteProposal(item.fileWrite, `${context}.fileWrite`)
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

function proposedActionApprovalStatus(action: AgentProposedAction) {
  switch (action.type) {
    case 'tool_call':
      return action.call.approvalStatus
    case 'mcp_tool_call':
      return action.approval.call.approvalStatus
    case 'builtin_capability_activation':
    case 'builtin_mcp_tool_approval':
    case 'browser_risk_approval':
      return action.approval.approvalStatus
    case 'diff':
      return action.diff.approvalStatus
    case 'file_write':
      return action.fileWrite.approvalStatus
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

function parseAgentFileWriteProposal(value: unknown, context: string): AgentFileWriteProposal {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'id',
      'draftId',
      'mode',
      'filePath',
      'baseRevision',
      'summary',
      'additions',
      'deletions',
      'lineCount',
      'byteCount',
      'approvalStatus'
    ] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    draftId: expectOpaqueRunId(item.draftId, `${context}.draftId`),
    mode: expectEnum(
      item.mode,
      ['create', 'rewrite', 'modify', 'append', 'upsert'] as const,
      `${context}.mode`
    ),
    filePath: expectBoundedString(item.filePath, `${context}.filePath`, 16 * 1024),
    baseRevision:
      item.baseRevision === null
        ? null
        : expectBoundedNonEmptyString(item.baseRevision, `${context}.baseRevision`, 2048),
    summary:
      item.summary === null
        ? null
        : expectBoundedString(item.summary, `${context}.summary`, 16 * 1024),
    additions: expectSafeInteger(item.additions, `${context}.additions`, 0),
    deletions: expectSafeInteger(item.deletions, `${context}.deletions`, 0),
    lineCount: expectSafeInteger(item.lineCount, `${context}.lineCount`, 0),
    byteCount: expectSafeInteger(item.byteCount, `${context}.byteCount`, 0),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`)
  }
}

function parseAgentCommandAction(value: unknown, context: string): AgentCommandActionProjection {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'id',
      'command',
      'cwd',
      'timeoutMs',
      'approvalStatus',
      'riskLevel',
      'reason',
      'observe'
    ] as const,
    context
  )
  const observe =
    item.observe === null
      ? null
      : parseAgentCommandObservationRequest(item.observe, `${context}.observe`)
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    command: expectBoundedString(item.command, `${context}.command`, 256 * 1024),
    cwd: item.cwd === null ? null : expectBoundedString(item.cwd, `${context}.cwd`, 16 * 1024),
    timeoutMs:
      item.timeoutMs === null ? null : expectSafeInteger(item.timeoutMs, `${context}.timeoutMs`, 0),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    riskLevel:
      item.riskLevel === null
        ? null
        : expectEnum(
            item.riskLevel,
            ['read_only', 'writes_workspace', 'network', 'destructive', 'unknown'] as const,
            `${context}.riskLevel`
          ),
    reason:
      item.reason === null
        ? null
        : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024),
    observe
  }
}

function parseAgentCommandObservationRequest(
  value: unknown,
  context: string
): NonNullable<AgentCommandActionProjection['observe']> {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['kinds', 'expectedOutputs', 'additionalRoots'] as const, context)
  const kinds = expectBoundedArray(item.kinds, `${context}.kinds`, 32).map((kind, index) =>
    expectEnum(kind, ['office'] as const, `${context}.kinds[${index}]`)
  )
  return {
    kinds,
    ...(item.expectedOutputs === undefined
      ? {}
      : {
          expectedOutputs: parseStringArray(
            item.expectedOutputs,
            `${context}.expectedOutputs`,
            1024,
            16 * 1024
          )
        }),
    ...(item.additionalRoots === undefined
      ? {}
      : {
          additionalRoots: parseStringArray(
            item.additionalRoots,
            `${context}.additionalRoots`,
            1024,
            16 * 1024
          )
        })
  }
}

function parseAgentSkillMaterializationRequest(
  value: unknown,
  context: string
): AgentSkillMaterializationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['id', 'sourceUri', 'sourcePrefix', 'destination', 'approvalStatus', 'reason'] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    sourceUri: expectBoundedString(item.sourceUri, `${context}.sourceUri`, 16 * 1024),
    sourcePrefix:
      item.sourcePrefix === null
        ? null
        : expectBoundedString(item.sourcePrefix, `${context}.sourcePrefix`, 16 * 1024),
    destination: expectBoundedString(item.destination, `${context}.destination`, 16 * 1024),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason:
      item.reason === null ? null : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

function parseAgentSkillScriptRequest(value: unknown, context: string): AgentSkillScriptRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'id',
      'scriptUri',
      'skillId',
      'skillRevision',
      'resourcePath',
      'resourceDigest',
      'interpreter',
      'args',
      'requirements',
      'preflight',
      'timeoutMs',
      'approvalStatus',
      'reason'
    ] as const,
    context
  )
  return {
    id: expectOpaqueRunId(item.id, `${context}.id`),
    scriptUri: expectBoundedString(item.scriptUri, `${context}.scriptUri`, 16 * 1024),
    skillId: expectOpaqueRunId(item.skillId, `${context}.skillId`),
    skillRevision: expectOpaqueRunId(item.skillRevision, `${context}.skillRevision`),
    resourcePath: expectBoundedString(item.resourcePath, `${context}.resourcePath`, 16 * 1024),
    resourceDigest: expectBoundedNonEmptyString(
      item.resourceDigest,
      `${context}.resourceDigest`,
      2048
    ),
    interpreter: expectEnum(item.interpreter, ['python3'] as const, `${context}.interpreter`),
    args: parseStringArray(item.args, `${context}.args`, 4096, 64 * 1024),
    requirements: parseAgentSkillScriptRequirements(item.requirements, `${context}.requirements`),
    preflight: parseAgentSkillScriptPreflight(item.preflight, `${context}.preflight`),
    timeoutMs:
      item.timeoutMs === null ? null : expectSafeInteger(item.timeoutMs, `${context}.timeoutMs`, 0),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason:
      item.reason === null ? null : expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

function parseAgentSkillScriptRequirements(
  value: unknown,
  context: string
): AgentSkillScriptRequest['requirements'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['pythonDistributions', 'commands'] as const, context)
  return {
    ...(item.pythonDistributions === undefined
      ? {}
      : {
          pythonDistributions: parseStringArray(
            item.pythonDistributions,
            `${context}.pythonDistributions`,
            4096,
            1024
          )
        }),
    ...(item.commands === undefined
      ? {}
      : {
          commands: parseStringArray(item.commands, `${context}.commands`, 4096, 1024)
        })
  }
}

function parseAgentSkillScriptPreflight(
  value: unknown,
  context: string
): AgentSkillScriptRequest['preflight'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'status',
      'interpreter',
      'interpreterVersion',
      'dependencies',
      'runtimeFingerprint',
      'errorCode',
      'message'
    ] as const,
    context
  )
  return {
    status: expectEnum(
      item.status,
      ['ready', 'missing_dependencies', 'unsupported', 'conflict'] as const,
      `${context}.status`
    ),
    interpreter: expectEnum(item.interpreter, ['python3'] as const, `${context}.interpreter`),
    ...(item.interpreterVersion === undefined
      ? {}
      : {
          interpreterVersion: expectBoundedNonEmptyString(
            item.interpreterVersion,
            `${context}.interpreterVersion`,
            1024
          )
        }),
    ...(item.dependencies === undefined
      ? {}
      : {
          dependencies: expectBoundedArray(item.dependencies, `${context}.dependencies`, 4096).map(
            (dependency, index) => {
              const dependencyContext = `${context}.dependencies[${index}]`
              const entry = expectRecord(dependency, dependencyContext)
              expectOnlyKeys(
                entry,
                ['kind', 'name', 'status', 'version'] as const,
                dependencyContext
              )
              return {
                kind: expectEnum(
                  entry.kind,
                  ['python_distribution', 'command'] as const,
                  `${dependencyContext}.kind`
                ),
                name: expectBoundedNonEmptyString(entry.name, `${dependencyContext}.name`, 1024),
                status: expectEnum(
                  entry.status,
                  ['available', 'missing'] as const,
                  `${dependencyContext}.status`
                ),
                ...(entry.version === undefined
                  ? {}
                  : {
                      version: expectBoundedNonEmptyString(
                        entry.version,
                        `${dependencyContext}.version`,
                        1024
                      )
                    })
              }
            }
          )
        }),
    runtimeFingerprint: expectBoundedNonEmptyString(
      item.runtimeFingerprint,
      `${context}.runtimeFingerprint`,
      4096
    ),
    ...(item.errorCode === undefined
      ? {}
      : {
          errorCode: expectBoundedNonEmptyString(item.errorCode, `${context}.errorCode`, 1024)
        }),
    ...(item.message === undefined
      ? {}
      : {
          message: expectBoundedString(
            item.message,
            `${context}.message`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          )
        })
  }
}

function parseAgentOfficeOperationRequest(
  value: unknown,
  context: string
): AgentOfficeOperationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['schemaVersion', 'id', 'semanticArgs', 'prepared', 'approvalStatus', 'reason'] as const,
    context
  )
  const semanticArgs = expectRecord(item.semanticArgs, `${context}.semanticArgs`)
  assertRendererSafeJson(semanticArgs, `${context}.semanticArgs`, 4 * 1024 * 1024)
  return {
    schemaVersion: expectExactSchemaVersion(item.schemaVersion, 6, `${context}.schemaVersion`),
    id: expectOpaqueRunId(item.id, `${context}.id`),
    semanticArgs,
    prepared: parseAgentOfficePreparedExecution(item.prepared, `${context}.prepared`),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    reason: expectBoundedString(item.reason, `${context}.reason`, 16 * 1024)
  }
}

function parseAgentOfficePreparedExecution(
  value: unknown,
  context: string
): AgentOfficeOperationRequest['prepared'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'schemaVersion',
      'providerId',
      'engineRevision',
      'workspaceRevision',
      'access',
      'request',
      'argv',
      'resolvedRenderPlan',
      'paths',
      'inputBindings'
    ] as const,
    context
  )
  assertRendererSafeJson(item, context, 4 * 1024 * 1024)
  expectExactSchemaVersion(item.schemaVersion, 6, `${context}.schemaVersion`)
  expectBoundedNonEmptyString(item.providerId, `${context}.providerId`, 1024)
  expectBoundedNonEmptyString(item.engineRevision, `${context}.engineRevision`, 4096)
  if (item.workspaceRevision !== null) {
    expectBoundedNonEmptyString(item.workspaceRevision, `${context}.workspaceRevision`, 4096)
  }
  expectEnum(item.access, ['readOnly', 'fileWrite'] as const, `${context}.access`)
  parseStringArray(item.argv, `${context}.argv`, 4096, 64 * 1024)
  if (item.resolvedRenderPlan !== null) {
    expectRecord(item.resolvedRenderPlan, `${context}.resolvedRenderPlan`)
  }
  expectBoundedArray(item.paths, `${context}.paths`, 4096)
  expectBoundedArray(item.inputBindings, `${context}.inputBindings`, 4096)

  const requestContext = `${context}.request`
  const request = expectRecord(item.request, requestContext)
  expectOnlyKeys(
    request,
    [
      'documentKind',
      'operation',
      'documentPath',
      'outputPath',
      'destinationPath',
      'inputs',
      'timeoutMs',
      'parameters'
    ] as const,
    requestContext
  )
  expectEnum(
    request.documentKind,
    ['document', 'spreadsheet', 'presentation'] as const,
    `${requestContext}.documentKind`
  )
  const operation = expectEnum(
    request.operation,
    [
      'help',
      'create',
      'view',
      'get',
      'query',
      'validate',
      'set',
      'add',
      'remove',
      'move',
      'swap'
    ] as const,
    `${requestContext}.operation`
  )
  for (const field of ['documentPath', 'outputPath', 'destinationPath'] as const) {
    if (request[field] !== null) {
      expectBoundedString(request[field], `${requestContext}.${field}`, 16 * 1024)
    }
  }
  expectBoundedArray(request.inputs, `${requestContext}.inputs`, 4096)
  if (request.timeoutMs !== null) {
    expectSafeInteger(request.timeoutMs, `${requestContext}.timeoutMs`, 0)
  }
  const parameters = expectRecord(request.parameters, `${requestContext}.parameters`)
  if (parameters.type !== operation) {
    throw invalidProtocolValue(
      `${requestContext}.parameters.type`,
      'must match the prepared Office operation'
    )
  }

  return JSON.parse(JSON.stringify(item)) as AgentOfficeOperationRequest['prepared']
}

function parseAgentSkillInstallationRequest(
  value: unknown,
  context: string
): AgentSkillInstallationRequest {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    ['schemaVersion', 'id', 'installRef', 'preview', 'approvalStatus', 'expiresAt'] as const,
    context
  )
  return {
    schemaVersion: expectExactSchemaVersion(item.schemaVersion, 1, `${context}.schemaVersion`),
    id: expectOpaqueRunId(item.id, `${context}.id`),
    installRef: expectBoundedString(item.installRef, `${context}.installRef`, 16 * 1024),
    preview: parseAgentSkillInstallationPreview(item.preview, `${context}.preview`),
    approvalStatus: parseAgentApprovalStatus(item.approvalStatus, `${context}.approvalStatus`),
    expiresAt: expectSafeInteger(item.expiresAt, `${context}.expiresAt`, 0)
  }
}

function parseAgentSkillInstallationPreview(
  value: unknown,
  context: string
): AgentSkillInstallationRequest['preview'] {
  const item = expectRecord(value, context)
  expectOnlyKeys(
    item,
    [
      'name',
      'description',
      'sourceSummary',
      'resolvedRevision',
      'fileCount',
      'totalBytes',
      'resourceSummary',
      'containsScripts',
      'warnings',
      'compatibility',
      'operation',
      'impact'
    ] as const,
    context
  )
  assertRendererSafeJson(item.sourceSummary, `${context}.sourceSummary`)
  const resourceSummary = expectRecord(item.resourceSummary, `${context}.resourceSummary`)
  expectOnlyKeys(
    resourceSummary,
    ['total', 'references', 'assets', 'scripts', 'bytes'] as const,
    `${context}.resourceSummary`
  )
  return {
    name: expectBoundedNonEmptyString(item.name, `${context}.name`, 1024),
    description: expectBoundedString(
      item.description,
      `${context}.description`,
      MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
    ),
    sourceSummary: item.sourceSummary,
    resolvedRevision: expectBoundedNonEmptyString(
      item.resolvedRevision,
      `${context}.resolvedRevision`,
      4096
    ),
    fileCount: expectSafeInteger(item.fileCount, `${context}.fileCount`, 0),
    totalBytes: expectSafeInteger(item.totalBytes, `${context}.totalBytes`, 0),
    resourceSummary: {
      total: expectSafeInteger(resourceSummary.total, `${context}.resourceSummary.total`, 0),
      references: expectSafeInteger(
        resourceSummary.references,
        `${context}.resourceSummary.references`,
        0
      ),
      assets: expectSafeInteger(resourceSummary.assets, `${context}.resourceSummary.assets`, 0),
      scripts: expectSafeInteger(resourceSummary.scripts, `${context}.resourceSummary.scripts`, 0),
      bytes: expectSafeInteger(resourceSummary.bytes, `${context}.resourceSummary.bytes`, 0)
    },
    containsScripts: expectBoolean(item.containsScripts, `${context}.containsScripts`),
    warnings: expectBoundedArray(item.warnings, `${context}.warnings`, 1024).map(
      (warning, index) => {
        const warningContext = `${context}.warnings[${index}]`
        const entry = expectRecord(warning, warningContext)
        expectOnlyKeys(
          entry,
          ['code', 'message', 'requiresAcknowledgement'] as const,
          warningContext
        )
        return {
          code: expectBoundedNonEmptyString(entry.code, `${warningContext}.code`, 1024),
          message: expectBoundedString(
            entry.message,
            `${warningContext}.message`,
            MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
          ),
          requiresAcknowledgement: expectBoolean(
            entry.requiresAcknowledgement,
            `${warningContext}.requiresAcknowledgement`
          )
        }
      }
    ),
    compatibility: expectBoundedNonEmptyString(
      item.compatibility,
      `${context}.compatibility`,
      1024
    ),
    operation: expectBoundedNonEmptyString(item.operation, `${context}.operation`, 1024),
    impact: expectBoundedNonEmptyString(item.impact, `${context}.impact`, 1024)
  }
}

function expectExactSchemaVersion(value: unknown, expected: number, context: string): number {
  const version = expectSafeInteger(value, context, 1)
  if (version !== expected) {
    throw invalidProtocolValue(context, `expected schema version ${expected}`)
  }
  return version
}

function parseStringArray(
  value: unknown,
  context: string,
  maximumItems: number,
  maximumItemBytes: number
): string[] {
  return expectBoundedArray(value, context, maximumItems).map((entry, index) =>
    expectBoundedString(entry, `${context}[${index}]`, maximumItemBytes)
  )
}

function parseAgentLlmRetryEvent(
  record: Record<string, unknown>
): Extract<AgentEvent, { type: 'llm_retry' }> {
  const context = 'LLM retry event'
  expectOnlyKeys(
    record,
    [
      'type',
      'runId',
      'streamId',
      'category',
      'providerCode',
      'delayMs',
      'retryAt',
      'attempt',
      'maxAttempts'
    ] as const,
    context
  )
  const rawCategory = expectBoundedNonEmptyString(record.category, `${context}.category`, 64)
  if (!LLM_RETRY_CATEGORIES.includes(rawCategory as AgentLlmRetryCategory)) {
    throw invalidProtocolValue(context, 'category must be a supported retry category')
  }
  const category = rawCategory as AgentLlmRetryCategory
  const delayMs = expectSafeInteger(record.delayMs, `${context}.delayMs`, 0)
  if (delayMs > MAX_LLM_RETRY_DELAY_MS) {
    throw invalidProtocolValue(context, `delayMs must not exceed ${MAX_LLM_RETRY_DELAY_MS}`)
  }
  const attempt = expectSafeInteger(record.attempt, `${context}.attempt`, 1)
  const maxAttempts = expectSafeInteger(record.maxAttempts, `${context}.maxAttempts`, 1)
  if (attempt > MAX_LLM_RETRY_ATTEMPTS || maxAttempts > MAX_LLM_RETRY_ATTEMPTS) {
    throw invalidProtocolValue(
      context,
      `attempt and maxAttempts must not exceed ${MAX_LLM_RETRY_ATTEMPTS}`
    )
  }
  if (attempt > maxAttempts) {
    throw invalidProtocolValue(context, 'attempt must not exceed maxAttempts')
  }
  const providerCode =
    record.providerCode === undefined
      ? undefined
      : expectBoundedNonEmptyString(record.providerCode, `${context}.providerCode`, 128)
  if (providerCode !== undefined && !CANONICAL_PROVIDER_CODE_PATTERN.test(providerCode)) {
    throw invalidProtocolValue(context, 'providerCode must be a bounded machine-readable code')
  }

  return {
    type: 'llm_retry',
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    streamId: expectBoundedNonEmptyString(record.streamId, `${context}.streamId`, 256),
    category,
    ...(providerCode === undefined ? {} : { providerCode }),
    delayMs,
    retryAt: expectSafeInteger(record.retryAt, `${context}.retryAt`, 0),
    attempt,
    maxAttempts
  }
}

function parseAgentMessageStreamResetEvent(
  record: Record<string, unknown>
): Extract<AgentEvent, { type: 'message_stream_reset' }> {
  const context = 'LLM message stream reset event'
  expectOnlyKeys(record, ['type', 'runId', 'streamId', 'reason'] as const, context)
  if (record.reason !== undefined) {
    // The reason can contain an upstream response body. It controls no Renderer behavior and is
    // replaced with a stable lifecycle code at the Host boundary.
    expectBoundedString(record.reason, `${context}.reason`, 16 * 1024)
  }
  return {
    type: 'message_stream_reset',
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    streamId: expectBoundedNonEmptyString(record.streamId, `${context}.streamId`, 256),
    reason: 'retrying_model_request'
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

export function parseAgentBuiltinCapabilityActivationProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'builtin_capability_activation' }> {
  const context = 'built-in capability activation proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'builtin_capability_activation') {
    throw invalidProtocolValue(context, 'type must be builtin_capability_activation')
  }
  return {
    type: 'builtin_capability_activation',
    approval: parseAgentBuiltinCapabilityActivationApproval(record.approval)
  }
}

export function parseAgentBuiltinCapabilityActivationApproval(
  value: unknown
): AgentBuiltinCapabilityActivationApproval {
  const context = 'built-in capability activation approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'actionId',
      'activationId',
      'runId',
      'callId',
      'capabilityId',
      'displayName',
      'reason',
      'manifestDigest',
      'policyRevision',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const activationId = expectUuidV4(record.activationId, `${context}.activationId`)
  if (actionId === activationId) {
    throw invalidProtocolValue(context, 'actionId and activationId must be distinct')
  }
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (createdAt > MAX_RENDERER_DATE_UNIX_SECONDS || expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS) {
    throw invalidProtocolValue(context, 'timestamps exceed the Renderer-safe date range')
  }
  if (expiresAt - createdAt !== BUILTIN_CAPABILITY_APPROVAL_TTL_SECONDS) {
    throw invalidProtocolValue(context, 'expiry must equal the fixed 15 minute approval TTL')
  }
  const reason = expectDisplayText(record.reason, `${context}.reason`, 4096)
  if (!reason.trim()) {
    throw invalidProtocolValue(context, 'reason must not be blank')
  }
  const displayName = expectDisplayText(record.displayName, `${context}.displayName`, 256)
  if (!displayName.trim()) {
    throw invalidProtocolValue(context, 'displayName must not be blank')
  }
  return {
    actionId,
    activationId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    displayName,
    reason,
    manifestDigest: expectVersionedSha256Digest(record.manifestDigest, `${context}.manifestDigest`),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0),
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

export function parseAgentBuiltinMcpToolApprovalProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'builtin_mcp_tool_approval' }> {
  const context = 'built-in MCP Tool approval proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'builtin_mcp_tool_approval') {
    throw invalidProtocolValue(context, 'type must be builtin_mcp_tool_approval')
  }
  return {
    type: 'builtin_mcp_tool_approval',
    approval: parseAgentBuiltinMcpToolApproval(record.approval)
  }
}

export function parseAgentBuiltinMcpToolApproval(value: unknown): AgentBuiltinMcpToolApproval {
  const context = 'built-in MCP Tool approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'identity',
      'capabilityDisplayName',
      'toolDisplayName',
      'callReason',
      'operationCategory',
      'resourceSummary',
      'riskKinds',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  if (record.schemaVersion !== 1) {
    throw invalidProtocolValue(`${context}.schemaVersion`, 'expected 1')
  }
  const identityRecord = expectRecord(record.identity, `${context}.identity`)
  expectOnlyKeys(
    identityRecord,
    [
      'actionId',
      'approvalId',
      'runId',
      'callId',
      'capabilityId',
      'capabilityActivationId',
      'managedMcpId',
      'packageName',
      'packageVersion',
      'upstreamCatalogDigest',
      'manifestDigest',
      'policyDigest',
      'policyRevision',
      'toolId',
      'rawName',
      'modelName',
      'upstreamSchemaDigest',
      'hostOverlayDigest',
      'hostInputSchemaDigest',
      'argumentsDigest',
      'resourceScopeDigest',
      'origin'
    ] as const,
    `${context}.identity`
  )
  const actionId = expectUuidV4(identityRecord.actionId, `${context}.identity.actionId`)
  const approvalId = expectUuidV4(identityRecord.approvalId, `${context}.identity.approvalId`)
  const capabilityActivationId = expectUuidV4(
    identityRecord.capabilityActivationId,
    `${context}.identity.capabilityActivationId`
  )
  if (new Set([actionId, approvalId, capabilityActivationId]).size !== 3) {
    throw invalidProtocolValue(context, 'approval identities must be distinct')
  }
  const origin = parseNullableHttpOrigin(identityRecord.origin, `${context}.identity.origin`)
  const resourceRecord = expectRecord(record.resourceSummary, `${context}.resourceSummary`)
  expectOnlyKeys(
    resourceRecord,
    ['scope', 'displayName', 'fileBasenames', 'origin'] as const,
    `${context}.resourceSummary`
  )
  if (!Array.isArray(resourceRecord.fileBasenames) || resourceRecord.fileBasenames.length > 32) {
    throw invalidProtocolValue(`${context}.resourceSummary.fileBasenames`, 'invalid file list')
  }
  const fileBasenames = resourceRecord.fileBasenames.map((value, index) => {
    const basename = expectDisplayText(
      value,
      `${context}.resourceSummary.fileBasenames[${index}]`,
      256
    )
    if (!basename.trim() || basename.includes('/') || basename.includes('\\')) {
      throw invalidProtocolValue(
        `${context}.resourceSummary.fileBasenames[${index}]`,
        'must remain a basename'
      )
    }
    return basename
  })
  const resourceOrigin = parseNullableHttpOrigin(
    resourceRecord.origin,
    `${context}.resourceSummary.origin`
  )
  if (resourceOrigin !== origin) {
    throw invalidProtocolValue(context, 'resource and identity origins must match')
  }
  if (!Array.isArray(record.riskKinds) || record.riskKinds.length === 0) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be a non-empty array')
  }
  const riskKinds = record.riskKinds.map((risk, index) =>
    expectEnum(risk, BUILTIN_MCP_TOOL_RISK_KINDS, `${context}.riskKinds[${index}]`)
  )
  if (
    new Set(riskKinds).size !== riskKinds.length ||
    riskKinds.some(
      (risk, index) =>
        index > 0 &&
        BUILTIN_MCP_TOOL_RISK_KINDS.indexOf(riskKinds[index - 1]) >=
          BUILTIN_MCP_TOOL_RISK_KINDS.indexOf(risk)
    )
  ) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be unique and canonical')
  }
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (
    createdAt > MAX_RENDERER_DATE_UNIX_SECONDS ||
    expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS ||
    expiresAt - createdAt !== BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS
  ) {
    throw invalidProtocolValue(context, 'invalid fixed approval TTL')
  }
  const rawName = expectBoundedNonEmptyString(
    identityRecord.rawName,
    `${context}.identity.rawName`,
    64
  )
  const modelName = expectBoundedNonEmptyString(
    identityRecord.modelName,
    `${context}.identity.modelName`,
    64
  )
  const toolId = expectBoundedNonEmptyString(
    identityRecord.toolId,
    `${context}.identity.toolId`,
    64
  )
  if (rawName !== modelName || rawName !== toolId || !SAFE_CODE_PATTERN.test(rawName)) {
    throw invalidProtocolValue(context, 'Tool identities must match the reviewed raw identity')
  }
  return {
    schemaVersion: 1,
    identity: {
      actionId,
      approvalId,
      runId: expectOpaqueRunId(identityRecord.runId, `${context}.identity.runId`),
      callId: expectModelToolCallId(identityRecord.callId, `${context}.identity.callId`),
      capabilityId: parseMcpBuiltinCapabilityId(
        identityRecord.capabilityId,
        `${context}.identity.capabilityId`
      ),
      capabilityActivationId,
      managedMcpId: expectBoundedNonEmptyString(
        identityRecord.managedMcpId,
        `${context}.identity.managedMcpId`,
        128
      ),
      packageName: expectDisplayText(
        identityRecord.packageName,
        `${context}.identity.packageName`,
        128
      ),
      packageVersion: expectDisplayText(
        identityRecord.packageVersion,
        `${context}.identity.packageVersion`,
        64
      ),
      upstreamCatalogDigest: expectVersionedSha256Digest(
        identityRecord.upstreamCatalogDigest,
        `${context}.identity.upstreamCatalogDigest`
      ),
      manifestDigest: expectVersionedSha256Digest(
        identityRecord.manifestDigest,
        `${context}.identity.manifestDigest`
      ),
      policyDigest: expectVersionedSha256Digest(
        identityRecord.policyDigest,
        `${context}.identity.policyDigest`
      ),
      policyRevision: expectSafeInteger(
        identityRecord.policyRevision,
        `${context}.identity.policyRevision`,
        1
      ),
      toolId,
      rawName,
      modelName,
      upstreamSchemaDigest: expectVersionedSha256Digest(
        identityRecord.upstreamSchemaDigest,
        `${context}.identity.upstreamSchemaDigest`
      ),
      hostOverlayDigest: expectVersionedSha256Digest(
        identityRecord.hostOverlayDigest,
        `${context}.identity.hostOverlayDigest`
      ),
      hostInputSchemaDigest: expectVersionedSha256Digest(
        identityRecord.hostInputSchemaDigest,
        `${context}.identity.hostInputSchemaDigest`
      ),
      argumentsDigest: expectVersionedSha256Digest(
        identityRecord.argumentsDigest,
        `${context}.identity.argumentsDigest`
      ),
      resourceScopeDigest: expectVersionedSha256Digest(
        identityRecord.resourceScopeDigest,
        `${context}.identity.resourceScopeDigest`
      ),
      origin
    },
    capabilityDisplayName: expectDisplayText(
      record.capabilityDisplayName,
      `${context}.capabilityDisplayName`,
      256
    ),
    toolDisplayName: expectDisplayText(record.toolDisplayName, `${context}.toolDisplayName`, 256),
    callReason: expectDisplayText(record.callReason, `${context}.callReason`, 4096),
    operationCategory: expectSafeCode(record.operationCategory, `${context}.operationCategory`),
    resourceSummary: {
      scope: expectSafeCode(resourceRecord.scope, `${context}.resourceSummary.scope`),
      displayName: expectDisplayText(
        resourceRecord.displayName,
        `${context}.resourceSummary.displayName`,
        512
      ),
      fileBasenames,
      origin: resourceOrigin
    },
    riskKinds,
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

export function parseAgentBrowserRiskProposedAction(
  value: unknown
): Extract<AgentProposedAction, { type: 'browser_risk_approval' }> {
  const context = 'browser risk proposed action'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'approval'] as const, context)
  if (record.type !== 'browser_risk_approval') {
    throw invalidProtocolValue(context, 'type must be browser_risk_approval')
  }
  return {
    type: 'browser_risk_approval',
    approval: parseAgentBrowserRiskApproval(record.approval)
  }
}

export function parseAgentBrowserRiskApproval(value: unknown): AgentBrowserRiskApproval {
  const context = 'browser risk approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'actionId',
      'riskApprovalId',
      'runId',
      'callId',
      'capabilityId',
      'capabilityActivationId',
      'displayName',
      'reason',
      'destination',
      'trigger',
      'triggerToolName',
      'riskKinds',
      'manifestDigest',
      'policyRevision',
      'createdAt',
      'expiresAt',
      'approvalStatus'
    ] as const,
    context
  )
  if (record.schemaVersion !== 1) {
    throw invalidProtocolValue(`${context}.schemaVersion`, 'expected 1')
  }

  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const riskApprovalId = expectUuidV4(record.riskApprovalId, `${context}.riskApprovalId`)
  const capabilityActivationId = expectUuidV4(
    record.capabilityActivationId,
    `${context}.capabilityActivationId`
  )
  if (
    actionId === riskApprovalId ||
    actionId === capabilityActivationId ||
    riskApprovalId === capabilityActivationId
  ) {
    throw invalidProtocolValue(
      context,
      'action, risk approval, and activation identities must differ'
    )
  }

  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (createdAt > MAX_RENDERER_DATE_UNIX_SECONDS || expiresAt > MAX_RENDERER_DATE_UNIX_SECONDS) {
    throw invalidProtocolValue(context, 'timestamps exceed the Renderer-safe date range')
  }
  if (expiresAt - createdAt !== BROWSER_RISK_APPROVAL_TTL_SECONDS) {
    throw invalidProtocolValue(context, 'expiry must equal the fixed 15 minute approval TTL')
  }

  const reason = expectDisplayText(record.reason, `${context}.reason`, 4096)
  const displayName = expectDisplayText(record.displayName, `${context}.displayName`, 256)
  if (!reason.trim()) throw invalidProtocolValue(context, 'reason must not be blank')
  if (!displayName.trim()) throw invalidProtocolValue(context, 'displayName must not be blank')

  if (!Array.isArray(record.riskKinds) || record.riskKinds.length === 0) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must be a non-empty array')
  }
  if (record.riskKinds.length > BROWSER_RISK_KINDS.length) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'exceeded the supported risk count')
  }
  const riskKinds = record.riskKinds.map((risk, index) =>
    expectEnum(risk, BROWSER_RISK_KINDS, `${context}.riskKinds[${index}]`)
  )
  if (new Set(riskKinds).size !== riskKinds.length) {
    throw invalidProtocolValue(`${context}.riskKinds`, 'must not contain duplicates')
  }

  return {
    schemaVersion: 1,
    actionId,
    riskApprovalId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
    capabilityActivationId,
    displayName,
    reason,
    destination: parseAgentBrowserDestinationIdentity(record.destination, `${context}.destination`),
    trigger: expectEnum(record.trigger, BROWSER_RISK_TRIGGERS, `${context}.trigger`),
    triggerToolName: expectEnum(
      record.triggerToolName,
      BROWSER_REVIEWED_TOOL_NAMES,
      `${context}.triggerToolName`
    ),
    riskKinds,
    manifestDigest: expectVersionedSha256Digest(record.manifestDigest, `${context}.manifestDigest`),
    policyRevision: expectSafeInteger(record.policyRevision, `${context}.policyRevision`, 0),
    createdAt,
    expiresAt,
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    )
  }
}

function parseAgentBrowserDestinationIdentity(
  value: unknown,
  context: string
): AgentBrowserRiskApproval['destination'] {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['normalizedUrl', 'origin', 'scheme', 'asciiHost', 'effectivePort', 'addressClass'] as const,
    context
  )
  const normalizedUrl = expectDisplayText(record.normalizedUrl, `${context}.normalizedUrl`, 2048)
  const origin = expectDisplayText(record.origin, `${context}.origin`, 512)
  const scheme = expectEnum(record.scheme, ['http', 'https'] as const, `${context}.scheme`)
  const asciiHost = expectBoundedNonEmptyString(record.asciiHost, `${context}.asciiHost`, 253)
  if (asciiHost !== asciiHost.toLowerCase() || hasAnyControl(asciiHost)) {
    throw invalidProtocolValue(`${context}.asciiHost`, 'must be canonical lower-case ASCII')
  }
  const effectivePort = expectSafeInteger(record.effectivePort, `${context}.effectivePort`, 1)
  if (effectivePort > 65_535) {
    throw invalidProtocolValue(`${context}.effectivePort`, 'must be a valid TCP port')
  }

  let parsed: URL
  try {
    parsed = new URL(normalizedUrl)
  } catch {
    throw invalidProtocolValue(`${context}.normalizedUrl`, 'must be an absolute URL')
  }
  const parsedHost = parsed.hostname.replace(/^\[|\]$/gu, '').toLowerCase()
  const parsedPort = parsed.port ? Number(parsed.port) : scheme === 'https' ? 443 : 80
  if (
    parsed.protocol !== `${scheme}:` ||
    parsedHost !== asciiHost ||
    parsedPort !== effectivePort ||
    parsed.origin !== origin ||
    parsed.username ||
    parsed.password ||
    parsed.search ||
    parsed.hash
  ) {
    throw invalidProtocolValue(context, 'normalized URL fields are inconsistent')
  }

  return {
    normalizedUrl,
    origin,
    scheme,
    asciiHost,
    effectivePort,
    addressClass: expectEnum(
      record.addressClass,
      BROWSER_ADDRESS_CLASSES,
      `${context}.addressClass`
    )
  }
}

/** Strict parser for durable Tool provenance crossing the Host-to-Renderer boundary. */
export function parseAgentToolIdentityForHost(value: unknown): AgentToolIdentity {
  const context = 'Agent Tool identity'
  const record = expectRecord(value, context)
  switch (record.type) {
    case 'builtin':
      expectOnlyKeys(record, ['type', 'toolName'] as const, context)
      return {
        type: 'builtin',
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    case 'runtime_extension':
      expectOnlyKeys(record, ['type', 'extensionId', 'toolName'] as const, context)
      return {
        type: 'runtime_extension',
        extensionId: expectBoundedNonEmptyString(record.extensionId, `${context}.extensionId`, 256),
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    case 'builtin_capability':
      expectOnlyKeys(
        record,
        [
          'type',
          'capabilityId',
          'managedMcpId',
          'packageName',
          'packageVersion',
          'upstreamCatalogDigest',
          'policyDigest',
          'manifestDigest',
          'toolId',
          'rawName',
          'modelName',
          'upstreamSchemaDigest',
          'hostOverlayDigest',
          'hostInputSchemaDigest'
        ] as const,
        context
      )
      return {
        type: 'builtin_capability',
        capabilityId: parseMcpBuiltinCapabilityId(record.capabilityId, `${context}.capabilityId`),
        managedMcpId: expectBuiltinCapabilityStableId(
          record.managedMcpId,
          `${context}.managedMcpId`
        ),
        packageName: expectBuiltinCapabilityPackageIdentity(
          record.packageName,
          `${context}.packageName`,
          256
        ),
        packageVersion: expectBuiltinCapabilityPackageIdentity(
          record.packageVersion,
          `${context}.packageVersion`,
          128
        ),
        upstreamCatalogDigest: expectVersionedSha256Digest(
          record.upstreamCatalogDigest,
          `${context}.upstreamCatalogDigest`
        ),
        policyDigest: expectVersionedSha256Digest(record.policyDigest, `${context}.policyDigest`),
        manifestDigest: expectVersionedSha256Digest(
          record.manifestDigest,
          `${context}.manifestDigest`
        ),
        toolId: expectBuiltinCapabilityStableId(record.toolId, `${context}.toolId`),
        rawName: expectBuiltinCapabilityStableId(record.rawName, `${context}.rawName`),
        modelName: expectBuiltinCapabilityModelName(record.modelName, `${context}.modelName`),
        upstreamSchemaDigest: expectVersionedSha256Digest(
          record.upstreamSchemaDigest,
          `${context}.upstreamSchemaDigest`
        ),
        hostOverlayDigest: expectVersionedSha256Digest(
          record.hostOverlayDigest,
          `${context}.hostOverlayDigest`
        ),
        hostInputSchemaDigest: expectVersionedSha256Digest(
          record.hostInputSchemaDigest,
          `${context}.hostInputSchemaDigest`
        )
      }
    case 'mcp':
      expectOnlyKeys(record, ['type', 'provenance'] as const, context)
      return {
        type: 'mcp',
        provenance: parseProvenance(record.provenance, `${context}.provenance`)
      }
    case 'unregistered':
      expectOnlyKeys(record, ['type', 'toolName'] as const, context)
      return {
        type: 'unregistered',
        toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 256)
      }
    default:
      throw invalidProtocolValue(context, 'type must be a supported Tool identity')
  }
}

export function parseAgentMcpToolApproval(value: unknown): AgentMcpToolApproval {
  const context = 'MCP Tool approval'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'identity',
      'call',
      'summary',
      'approvalMode',
      'payloadPersistence',
      'createdAt',
      'expiresAt'
    ] as const,
    context
  )
  const identity = parseInvocationIdentity(record.identity, `${context}.identity`)
  const call = parseMcpToolCall(record.call, `${context}.call`)
  const summary = parseApprovalSummary(record.summary, `${context}.summary`)
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const expiresAt = expectSafeInteger(record.expiresAt, `${context}.expiresAt`, 0)
  if (expiresAt <= createdAt || expiresAt - createdAt > MCP_APPROVAL_TTL_MS) {
    throw invalidProtocolValue(context, 'expiry must be within the fixed 15 minute approval TTL')
  }
  if (call.id !== identity.callId || call.tool !== identity.provenance.modelToolName) {
    throw invalidProtocolValue(context, 'safe call projection must match invocation identity')
  }
  if (
    summary.serverId !== identity.provenance.serverId ||
    summary.rawToolName !== identity.provenance.rawToolName ||
    summary.modelToolName !== identity.provenance.modelToolName ||
    !sameScope(summary.scope, identity.provenance.scope)
  ) {
    throw invalidProtocolValue(context, 'summary must match frozen provenance')
  }
  const approvalMode = expectEnum(
    record.approvalMode,
    ['prompt', 'auto', 'deny'] as const,
    `${context}.approvalMode`
  )
  if (
    !(
      (approvalMode === 'prompt' && call.approvalStatus === 'required') ||
      (approvalMode === 'auto' && call.approvalStatus === 'approved')
    ) ||
    call.reason !== null
  ) {
    throw invalidProtocolValue(context, 'approval mode and call status must remain consistent')
  }
  return {
    identity,
    call,
    summary,
    approvalMode,
    payloadPersistence: expectEnum(
      record.payloadPersistence,
      ['process_only', 'durable_authenticated_envelope'] as const,
      `${context}.payloadPersistence`
    ),
    createdAt,
    expiresAt
  }
}

export function parseAgentMcpToolInvocationEvent(value: unknown): AgentMcpToolInvocationEvent {
  const context = 'MCP Tool invocation event'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'actionId',
      'invocationId',
      'callId',
      'serverId',
      'serverDisplayName',
      'rawToolName',
      'modelToolName',
      'displayReason',
      'external',
      'state',
      'dispatchCertainty',
      'outcome',
      'isError',
      'errorCode',
      'durationMs',
      'outputTruncated',
      'diagnostics'
    ] as const,
    context
  )
  for (const key of [
    'displayReason',
    'outcome',
    'isError',
    'errorCode',
    'durationMs',
    'diagnostics'
  ] as const) {
    if (!Object.hasOwn(record, key)) {
      throw invalidProtocolValue(context, `${key} is required`)
    }
  }
  const outcome =
    record.outcome === null
      ? null
      : expectEnum(
          record.outcome,
          [
            'succeeded',
            'tool_error',
            'output_too_large',
            'transport_error',
            'timed_out',
            'cancelled',
            'rejected',
            'expired',
            'payload_unavailable',
            'policy_denied',
            'outcome_unknown'
          ] as const,
          `${context}.outcome`
        )
  const isError =
    record.isError === null ? null : expectBoolean(record.isError, `${context}.isError`)
  const errorCode =
    record.errorCode === null ? null : expectSafeCode(record.errorCode, `${context}.errorCode`)
  const durationMs =
    record.durationMs === null
      ? null
      : expectSafeInteger(record.durationMs, `${context}.durationMs`, 0)
  const displayReason =
    record.displayReason === null
      ? null
      : expectDisplayText(record.displayReason, `${context}.displayReason`, 512)
  const external = expectBoolean(record.external, `${context}.external`)
  if (!external) {
    throw invalidProtocolValue(context, 'external must be true')
  }
  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const invocationId = expectUuidV4(record.invocationId, `${context}.invocationId`)
  if (actionId === invocationId) {
    throw invalidProtocolValue(context, 'actionId and invocationId must be distinct')
  }
  const state = expectEnum(
    record.state,
    [
      'pending_approval',
      'approved',
      'dispatching',
      'running',
      'completed',
      'failed',
      'cancelled',
      'rejected',
      'expired',
      'payload_unavailable',
      'policy_denied',
      'outcome_unknown'
    ] as const,
    `${context}.state`
  )
  const dispatchCertainty = expectEnum(
    record.dispatchCertainty,
    ['definitely_not_dispatched', 'possibly_dispatched', 'response_received'] as const,
    `${context}.dispatchCertainty`
  )
  const outputTruncated = expectBoolean(record.outputTruncated, `${context}.outputTruncated`)
  const diagnostics =
    record.diagnostics === null
      ? null
      : parseAgentMcpInvocationDiagnostics(record.diagnostics, `${context}.diagnostics`)
  assertValidInvocationLifecycle(
    context,
    state,
    dispatchCertainty,
    outcome ?? undefined,
    isError ?? undefined,
    errorCode ?? undefined,
    durationMs ?? undefined,
    outputTruncated
  )
  if (diagnostics !== null) {
    assertValidInvocationDiagnostics(context, state, outcome ?? undefined, diagnostics)
  }
  return {
    actionId,
    invocationId,
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    serverDisplayName: expectServerDisplayName(
      record.serverDisplayName,
      `${context}.serverDisplayName`
    ),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    displayReason,
    external,
    state,
    dispatchCertainty,
    outcome,
    isError,
    errorCode,
    durationMs,
    outputTruncated,
    diagnostics
  }
}

function parseAgentMcpInvocationDiagnostics(
  value: unknown,
  context: string
): AgentMcpInvocationDiagnostics {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'argumentEncodedBytes',
      'argumentValueCount',
      'argumentMaxDepth',
      'result',
      'failureStage'
    ] as const,
    context
  )
  const schemaVersion = boundedDiagnosticInteger(
    record.schemaVersion,
    `${context}.schemaVersion`,
    1
  )
  if (schemaVersion !== 1) {
    throw invalidProtocolValue(context, 'unsupported diagnostics schema version')
  }
  const result =
    record.result === undefined
      ? undefined
      : parseAgentMcpResultSizeSummary(record.result, `${context}.result`)
  const failureStage =
    record.failureStage === undefined
      ? undefined
      : expectEnum(
          record.failureStage,
          [
            'preflight',
            'approval_payload',
            'policy',
            'dispatch',
            'transport',
            'server_response',
            'result_projection',
            'persistence',
            'shutdown'
          ] as const,
          `${context}.failureStage`
        )
  return {
    schemaVersion: 1,
    argumentEncodedBytes: boundedDiagnosticInteger(
      record.argumentEncodedBytes,
      `${context}.argumentEncodedBytes`,
      MAX_MCP_DIAGNOSTIC_ARGUMENT_BYTES
    ),
    argumentValueCount: boundedDiagnosticInteger(
      record.argumentValueCount,
      `${context}.argumentValueCount`,
      MAX_MCP_DIAGNOSTIC_ARGUMENT_VALUES
    ),
    argumentMaxDepth: boundedDiagnosticInteger(
      record.argumentMaxDepth,
      `${context}.argumentMaxDepth`,
      MAX_MCP_DIAGNOSTIC_ARGUMENT_DEPTH
    ),
    ...(result === undefined ? {} : { result }),
    ...(failureStage === undefined ? {} : { failureStage })
  }
}

function parseAgentMcpResultSizeSummary(value: unknown, context: string) {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'contentBlockCount',
      'textBytes',
      'structuredBytes',
      'omittedBlockCount',
      'omittedEncodedBytes'
    ] as const,
    context
  )
  const contentBlockCount = boundedDiagnosticInteger(
    record.contentBlockCount,
    `${context}.contentBlockCount`,
    MAX_MCP_DIAGNOSTIC_RESULT_BLOCKS
  )
  const omittedBlockCount = boundedDiagnosticInteger(
    record.omittedBlockCount,
    `${context}.omittedBlockCount`,
    MAX_MCP_DIAGNOSTIC_RESULT_BLOCKS
  )
  if (omittedBlockCount > contentBlockCount) {
    throw invalidProtocolValue(context, 'omitted block count exceeds content block count')
  }
  return {
    contentBlockCount,
    textBytes: boundedDiagnosticInteger(
      record.textBytes,
      `${context}.textBytes`,
      MAX_MCP_DIAGNOSTIC_RESULT_BYTES
    ),
    structuredBytes: boundedDiagnosticInteger(
      record.structuredBytes,
      `${context}.structuredBytes`,
      MAX_MCP_DIAGNOSTIC_RESULT_BYTES
    ),
    omittedBlockCount,
    omittedEncodedBytes: boundedDiagnosticInteger(
      record.omittedEncodedBytes,
      `${context}.omittedEncodedBytes`,
      MAX_MCP_DIAGNOSTIC_RESULT_BYTES
    )
  }
}

function boundedDiagnosticInteger(value: unknown, context: string, maximum: number): number {
  const parsed = expectSafeInteger(value, context, 0)
  if (parsed > maximum) {
    throw invalidProtocolValue(context, `exceeded maximum ${maximum}`)
  }
  return parsed
}

function assertValidInvocationDiagnostics(
  context: string,
  state: AgentMcpToolInvocationEvent['state'],
  outcome: Exclude<AgentMcpToolInvocationEvent['outcome'], null> | undefined,
  diagnostics: AgentMcpInvocationDiagnostics
): void {
  const hasResult = diagnostics.result !== undefined
  const stage = diagnostics.failureStage
  const valid = (() => {
    switch (state) {
      case 'pending_approval':
      case 'approved':
      case 'dispatching':
      case 'running':
      case 'rejected':
        return !hasResult && stage === undefined
      case 'completed':
        return (
          hasResult &&
          ((outcome === 'succeeded' && stage === undefined) ||
            (outcome === 'tool_error' && stage === 'server_response'))
        )
      case 'expired':
      case 'payload_unavailable':
        return !hasResult && stage === 'approval_payload'
      case 'policy_denied':
        return !hasResult && stage === 'policy'
      case 'cancelled':
        return !hasResult && (stage === undefined || stage === 'preflight')
      case 'failed':
      case 'outcome_unknown':
        return !hasResult && stage !== undefined
    }
  })()
  if (!valid) {
    throw invalidProtocolValue(context, 'diagnostics contradict the invocation lifecycle state')
  }
}

function assertValidInvocationLifecycle(
  context: string,
  state: AgentMcpToolInvocationEvent['state'],
  dispatchCertainty: AgentMcpToolInvocationEvent['dispatchCertainty'],
  outcome: Exclude<AgentMcpToolInvocationEvent['outcome'], null> | undefined,
  isError: boolean | undefined,
  errorCode: string | undefined,
  durationMs: number | undefined,
  outputTruncated: boolean
): void {
  let valid = false
  switch (state) {
    case 'pending_approval':
    case 'approved':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      break
    case 'dispatching':
    case 'running':
      valid =
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === undefined &&
        isError === undefined &&
        errorCode === undefined &&
        durationMs === undefined &&
        !outputTruncated
      break
    case 'completed':
      valid =
        dispatchCertainty === 'response_received' &&
        durationMs !== undefined &&
        ((outcome === 'succeeded' && isError === false && errorCode === undefined) ||
          (outcome === 'tool_error' && isError === true && errorCode !== undefined))
      break
    case 'failed':
      valid =
        durationMs !== undefined &&
        errorCode !== undefined &&
        isError === true &&
        ((outcome === 'output_too_large' && dispatchCertainty === 'response_received') ||
          (outcome === 'timed_out' &&
            dispatchCertainty === 'definitely_not_dispatched' &&
            !outputTruncated) ||
          (outcome === 'transport_error' &&
            (dispatchCertainty === 'definitely_not_dispatched' ||
              dispatchCertainty === 'response_received') &&
            (!outputTruncated || dispatchCertainty === 'response_received')))
      break
    case 'cancelled':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'cancelled' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      break
    case 'rejected':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'rejected' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      break
    case 'expired':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'expired' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      break
    case 'payload_unavailable':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'payload_unavailable' &&
        isError === true &&
        errorCode !== undefined &&
        !outputTruncated
      break
    case 'policy_denied':
      valid =
        dispatchCertainty === 'definitely_not_dispatched' &&
        outcome === 'policy_denied' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      break
    case 'outcome_unknown':
      valid =
        dispatchCertainty === 'possibly_dispatched' &&
        outcome === 'outcome_unknown' &&
        isError === undefined &&
        errorCode !== undefined &&
        !outputTruncated
      break
  }
  if (!valid) {
    throw invalidProtocolValue(context, 'state, outcome, and dispatch certainty are inconsistent')
  }
}

export function parsePendingAgentActionSnapshotsForHost(
  value: unknown
): PendingAgentActionSnapshot[] {
  if (!Array.isArray(value)) {
    throw invalidProtocolValue('pending Agent actions', 'expected an array')
  }
  if (value.length > 1024) {
    throw invalidProtocolValue('pending Agent actions', 'action count exceeded 1024')
  }
  return value.map((entry, index) => {
    const record = expectRecord(entry, `pending Agent actions[${index}]`)
    const action = expectRecord(record.action, `pending Agent actions[${index}].action`)
    if (
      action.type !== 'mcp_tool_call' &&
      action.type !== 'builtin_capability_activation' &&
      action.type !== 'builtin_mcp_tool_approval' &&
      action.type !== 'browser_risk_approval'
    ) {
      return entry as PendingAgentActionSnapshot
    }
    if (action.type === 'builtin_capability_activation') {
      const context = `pending built-in capability Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBuiltinCapabilityActivationProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (parsedAction.approval.actionId !== actionId || parsedAction.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'pending identity must match capability approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending capability approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match capability approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'builtin_capability_activation',
          `${context}.actionType`
        ),
        toolName: expectExactString(record.toolName, 'activate_capability', `${context}.toolName`),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    if (action.type === 'browser_risk_approval') {
      const context = `pending browser risk Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBrowserRiskProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (parsedAction.approval.actionId !== actionId || parsedAction.approval.runId !== runId) {
        throw invalidProtocolValue(context, 'pending identity must match browser risk approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending browser risk approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match browser risk approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'browser_risk_approval',
          `${context}.actionType`
        ),
        toolName: expectExactString(
          record.toolName,
          parsedAction.approval.triggerToolName,
          `${context}.toolName`
        ),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    if (action.type === 'builtin_mcp_tool_approval') {
      const context = `pending built-in MCP Tool Agent action[${index}]`
      expectOnlyKeys(
        record,
        [
          'actionId',
          'actionType',
          'toolName',
          'toolCallId',
          'runId',
          'conversationId',
          'assistantMessageId',
          'action',
          'createdAt',
          'status'
        ] as const,
        context
      )
      const parsedAction = parseAgentBuiltinMcpToolApprovalProposedAction(action)
      const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
      const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
      if (
        parsedAction.approval.identity.actionId !== actionId ||
        parsedAction.approval.identity.runId !== runId
      ) {
        throw invalidProtocolValue(context, 'pending identity must match built-in MCP approval')
      }
      if (parsedAction.approval.approvalStatus !== 'required') {
        throw invalidProtocolValue(context, 'pending built-in MCP approval must remain required')
      }
      const toolCallId = expectModelToolCallId(record.toolCallId, `${context}.toolCallId`)
      if (toolCallId !== parsedAction.approval.identity.callId) {
        throw invalidProtocolValue(context, 'toolCallId must match built-in MCP approval')
      }
      expectExactString(record.status, 'pending', `${context}.status`)
      return {
        actionId,
        actionType: expectExactString(
          record.actionType,
          'builtin_mcp_tool_approval',
          `${context}.actionType`
        ),
        toolName: expectExactString(
          record.toolName,
          parsedAction.approval.identity.rawName,
          `${context}.toolName`
        ),
        toolCallId,
        runId,
        conversationId: parseOptionalNullableString(
          record.conversationId,
          `${context}.conversationId`
        ),
        assistantMessageId: parseOptionalNullableString(
          record.assistantMessageId,
          `${context}.assistantMessageId`
        ),
        action: parsedAction,
        createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
        status: 'pending'
      }
    }
    const context = `pending MCP Agent action[${index}]`
    expectOnlyKeys(
      record,
      [
        'actionId',
        'actionType',
        'toolName',
        'toolCallId',
        'runId',
        'conversationId',
        'assistantMessageId',
        'action',
        'createdAt',
        'status'
      ] as const,
      context
    )
    const parsedAction = parseAgentMcpProposedAction(action)
    const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
    const runId = expectOpaqueRunId(record.runId, `${context}.runId`)
    if (
      parsedAction.approval.identity.actionId !== actionId ||
      parsedAction.approval.identity.runId !== runId
    ) {
      throw invalidProtocolValue(context, 'pending identity must match MCP approval')
    }
    const toolCallId = parseOptionalNullableString(record.toolCallId, `${context}.toolCallId`)
    if (toolCallId !== null && toolCallId !== parsedAction.approval.identity.callId) {
      throw invalidProtocolValue(context, 'toolCallId must match MCP approval')
    }
    return {
      actionId,
      actionType: expectExactString(record.actionType, 'mcp_tool_call', `${context}.actionType`),
      toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 64),
      toolCallId,
      runId,
      conversationId: parseOptionalNullableString(
        record.conversationId,
        `${context}.conversationId`
      ),
      assistantMessageId: parseOptionalNullableString(
        record.assistantMessageId,
        `${context}.assistantMessageId`
      ),
      action: parsedAction,
      createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
      status: expectEnum(
        record.status,
        [
          'pending',
          'approved',
          'executing',
          'rejected',
          'cancelled',
          'completed',
          'failed'
        ] as const,
        `${context}.status`
      )
    }
  })
}

/**
 * MCP action execution has a dedicated safe projection. Generic Tool result bodies, exact trace
 * records and Tool definitions are intentionally not forwarded to Renderer.
 */
export function parseAgentActionExecutionOutputForHost(value: unknown): AgentActionExecutionOutput {
  const record = expectRecord(value, 'Agent action execution output')
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
  expectOnlyKeys(
    record,
    [
      'actionId',
      'actionType',
      'toolName',
      'status',
      'patchResult',
      'fileWriteResult',
      'commandResult',
      'toolResult',
      'agentOutput'
    ] as const,
    context
  )
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

function parseInvocationIdentity(value: unknown, context: string): AgentMcpToolInvocationIdentity {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['actionId', 'invocationId', 'runId', 'callId', 'provenance'] as const,
    context
  )
  const actionId = expectUuidV4(record.actionId, `${context}.actionId`)
  const invocationId = expectUuidV4(record.invocationId, `${context}.invocationId`)
  if (actionId === invocationId) {
    throw invalidProtocolValue(context, 'actionId and invocationId must be distinct')
  }
  return {
    actionId,
    invocationId,
    runId: expectOpaqueRunId(record.runId, `${context}.runId`),
    callId: expectModelToolCallId(record.callId, `${context}.callId`),
    provenance: parseProvenance(record.provenance, `${context}.provenance`)
  }
}

function parseProvenance(value: unknown, context: string): AgentMcpToolProvenance {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'serverId',
      'scope',
      'rawToolName',
      'modelToolName',
      'configEpoch',
      'registryRevision',
      'configDigest',
      'catalogGeneration',
      'catalogDigest',
      'catalogSchemaDigest',
      'schemaDigest',
      'schemaNormalizerVersion'
    ] as const,
    context
  )
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    scope: parseScope(record.scope, `${context}.scope`),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    configEpoch: expectUuidV4(record.configEpoch, `${context}.configEpoch`),
    registryRevision: expectSafeInteger(record.registryRevision, `${context}.registryRevision`, 0),
    configDigest: expectDigest(record.configDigest, `${context}.configDigest`),
    catalogGeneration: expectSafeInteger(
      record.catalogGeneration,
      `${context}.catalogGeneration`,
      0
    ),
    catalogDigest: expectDigest(record.catalogDigest, `${context}.catalogDigest`),
    catalogSchemaDigest: expectDigest(record.catalogSchemaDigest, `${context}.catalogSchemaDigest`),
    schemaDigest: expectDigest(record.schemaDigest, `${context}.schemaDigest`),
    schemaNormalizerVersion: expectSafeInteger(
      record.schemaNormalizerVersion,
      `${context}.schemaNormalizerVersion`,
      1
    )
  }
}

function parseApprovalSummary(value: unknown, context: string): AgentMcpToolApprovalSummary {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'serverId',
      'serverDisplayName',
      'scope',
      'rawToolName',
      'modelToolName',
      'displayReason',
      'arguments',
      'risk',
      'external'
    ] as const,
    context
  )
  const external = expectBoolean(record.external, `${context}.external`)
  if (!external) {
    throw invalidProtocolValue(context, 'external must be true')
  }
  if (!Object.hasOwn(record, 'displayReason')) {
    throw invalidProtocolValue(context, 'displayReason is required')
  }
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    serverDisplayName: expectServerDisplayName(
      record.serverDisplayName,
      `${context}.serverDisplayName`
    ),
    scope: parseScope(record.scope, `${context}.scope`),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    displayReason:
      record.displayReason === null
        ? null
        : expectDisplayText(record.displayReason, `${context}.displayReason`, 512),
    arguments: parseArgumentSummary(record.arguments, `${context}.arguments`),
    risk: expectEnum(
      record.risk,
      [
        'unknown',
        'read_only_claimed',
        'side_effects_possible',
        'destructive_claimed',
        'open_world_claimed'
      ] as const,
      `${context}.risk`
    ),
    external
  }
}

function parseArgumentSummary(value: unknown, context: string): AgentMcpArgumentSummary {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'encodedBytes',
      'topLevelPropertyCount',
      'stringValueCount',
      'numberValueCount',
      'booleanValueCount',
      'nullValueCount',
      'objectValueCount',
      'arrayValueCount',
      'maxDepth',
      'truncated'
    ] as const,
    context
  )
  return {
    encodedBytes: expectSafeInteger(record.encodedBytes, `${context}.encodedBytes`, 0),
    topLevelPropertyCount: expectSafeInteger(
      record.topLevelPropertyCount,
      `${context}.topLevelPropertyCount`,
      0
    ),
    stringValueCount: expectSafeInteger(record.stringValueCount, `${context}.stringValueCount`, 0),
    numberValueCount: expectSafeInteger(record.numberValueCount, `${context}.numberValueCount`, 0),
    booleanValueCount: expectSafeInteger(
      record.booleanValueCount,
      `${context}.booleanValueCount`,
      0
    ),
    nullValueCount: expectSafeInteger(record.nullValueCount, `${context}.nullValueCount`, 0),
    objectValueCount: expectSafeInteger(record.objectValueCount, `${context}.objectValueCount`, 0),
    arrayValueCount: expectSafeInteger(record.arrayValueCount, `${context}.arrayValueCount`, 0),
    maxDepth: expectSafeInteger(record.maxDepth, `${context}.maxDepth`, 0),
    truncated: expectBoolean(record.truncated, `${context}.truncated`)
  }
}

function parseMcpToolCall(value: unknown, context: string): AgentToolCall {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['id', 'tool', 'args', 'approvalStatus', 'reason'] as const, context)
  const args = expectRecord(record.args, `${context}.args`)
  expectOnlyKeys(args, [] as const, `${context}.args`)
  if (!Object.hasOwn(record, 'reason')) {
    throw invalidProtocolValue(context, 'reason is required')
  }
  const reason =
    record.reason === null ? null : expectDisplayText(record.reason, `${context}.reason`, 1024)
  return {
    id: expectModelToolCallId(record.id, `${context}.id`),
    tool: expectBoundedNonEmptyString(record.tool, `${context}.tool`, 64),
    args: {},
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    ),
    reason
  }
}

function parseMcpAgentChatOutput(value: unknown, context: string): AgentChatOutput {
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
      ['idle', 'running', 'waiting_for_approval', 'completed', 'failed', 'cancelled'] as const,
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

function parseStrictProposedActions(
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

function parseScope(value: unknown, context: string): AgentMcpServerScope {
  const record = expectRecord(value, context)
  switch (record.type) {
    case 'builtin':
    case 'user':
    case 'managed':
      expectOnlyKeys(record, ['type'] as const, context)
      return { type: record.type }
    case 'project':
      expectOnlyKeys(record, ['type', 'projectId'] as const, context)
      return {
        type: 'project',
        projectId: expectBoundedNonEmptyString(record.projectId, `${context}.projectId`, 256)
      }
    case 'plugin':
      expectOnlyKeys(record, ['type', 'pluginId'] as const, context)
      return {
        type: 'plugin',
        pluginId: expectBoundedNonEmptyString(record.pluginId, `${context}.pluginId`, 256)
      }
    default:
      throw invalidProtocolValue(context, `unknown type ${String(record.type)}`)
  }
}

function sameScope(left: AgentMcpServerScope, right: AgentMcpServerScope): boolean {
  if (left.type !== right.type) return false
  if (left.type === 'project' && right.type === 'project') {
    return left.projectId === right.projectId
  }
  if (left.type === 'plugin' && right.type === 'plugin') {
    return left.pluginId === right.pluginId
  }
  return true
}

function expectDisplayText(value: unknown, context: string, maxBytes: number): string {
  const result = expectBoundedString(value, context, maxBytes)
  if (hasDisallowedDisplayControl(result)) {
    throw invalidProtocolValue(context, 'contains disallowed control characters')
  }
  return result
}

function expectServerDisplayName(value: unknown, context: string): string {
  const result = expectBoundedString(value, context, 256)
  if (result.length === 0 || result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed display name without controls')
  }
  return result
}

function expectOpaqueRunId(value: unknown, context: string): string {
  const result = expectBoundedString(value, context, 2048)
  if (result.length === 0 || result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed opaque run identity without controls')
  }
  return result
}

function expectModelToolCallId(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!MODEL_TOOL_CALL_ID_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a canonical application-owned Tool Call id')
  }
  return result
}

function hasDisallowedDisplayControl(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value)
}

function hasAnyControl(value: string): boolean {
  return /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(value)
}

function expectBoundedNonEmptyString(value: unknown, context: string, maxBytes: number): string {
  const result = expectBoundedString(value, context, maxBytes)
  if (result.length === 0) {
    throw invalidProtocolValue(context, 'must not be empty')
  }
  return result
}

function expectBoundedString(value: unknown, context: string, maxBytes: number): string {
  const result = expectString(value, context)
  if (new TextEncoder().encode(result).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `exceeded ${maxBytes} UTF-8 bytes`)
  }
  return result
}

function expectSafeCode(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!SAFE_CODE_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a bounded safe code')
  }
  return result
}

function parseNullableHttpOrigin(value: unknown, context: string): string | null {
  if (value === null) return null
  const origin = expectDisplayText(value, context, 2048)
  let parsed: URL
  try {
    parsed = new URL(origin)
  } catch {
    throw invalidProtocolValue(context, 'must be an HTTP(S) origin')
  }
  if (
    !['http:', 'https:'].includes(parsed.protocol) ||
    parsed.origin !== origin ||
    parsed.username !== '' ||
    parsed.password !== '' ||
    parsed.pathname !== '/' ||
    parsed.search !== '' ||
    parsed.hash !== ''
  ) {
    throw invalidProtocolValue(context, 'must be a canonical HTTP(S) origin')
  }
  return origin
}

function expectExactString(value: unknown, expected: string, context: string): string {
  if (value !== expected) {
    throw invalidProtocolValue(context, `expected ${expected}`)
  }
  return expected
}

function parseOptionalNullableString(value: unknown, context: string): string | null {
  if (value === undefined || value === null) return null
  return expectBoundedNonEmptyString(value, context, 256)
}

function expectUuid(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result) ||
    result === '00000000-0000-0000-0000-000000000000'
  ) {
    throw invalidProtocolValue(context, 'expected a canonical non-nil lower-case UUID')
  }
  return result
}

function expectUuidV4(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!UUID_V4_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a canonical lower-case UUIDv4')
  }
  return result
}

function expectDigest(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!DIGEST_PATTERN.test(result)) {
    throw invalidProtocolValue(context, 'expected a lower-case SHA-256 digest')
  }
  return result
}

function expectVersionedSha256Digest(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!/^sha256:[0-9a-f]{64}$/.test(result)) {
    throw invalidProtocolValue(context, 'expected a sha256:-prefixed lower-case digest')
  }
  return result
}

function expectBuiltinCapabilityStableId(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    result.length === 0 ||
    new TextEncoder().encode(result).byteLength > 128 ||
    !/^[a-z](?:[a-z0-9._-]*[a-z0-9])?$/.test(result) ||
    result.includes('..')
  ) {
    throw invalidProtocolValue(context, 'expected a stable lower-case built-in capability id')
  }
  return result
}

function expectBuiltinCapabilityModelName(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    result.length === 0 ||
    new TextEncoder().encode(result).byteLength > 64 ||
    !/^[A-Za-z0-9_-]+$/.test(result)
  ) {
    throw invalidProtocolValue(context, 'expected a bounded Provider-visible Tool name')
  }
  return result
}

function expectBuiltinCapabilityPackageIdentity(
  value: unknown,
  context: string,
  maxBytes: number
): string {
  const result = expectBoundedNonEmptyString(value, context, maxBytes)
  if (result.trim() !== result || hasAnyControl(result)) {
    throw invalidProtocolValue(context, 'expected a trimmed package identity without controls')
  }
  return result
}
