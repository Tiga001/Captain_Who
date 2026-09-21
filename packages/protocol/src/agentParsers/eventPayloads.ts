import type {
  AgentContextWindowSnapshot,
  AgentEvent,
  AgentLlmRetryCategory,
  AgentStateSnapshot,
  AgentTodoState,
  AgentToolDefinition,
  AgentToolCall,
  AgentToolResult,
  AgentUsage,
  ConversationTraceAttachment
} from '../agent'
import type { AgentFolderReference } from '../attachments'
import type { ActivatedSkillSummary } from '../skills'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import { parseAgentToolIdentityForHost } from './mcpIdentity'
import {
  CANONICAL_PROVIDER_CODE_PATTERN,
  MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES,
  assertRendererSafeJson,
  expectBoundedArray,
  expectBoundedNonEmptyString,
  expectBoundedString,
  expectDisplayText,
  expectModelToolCallId,
  expectOpaqueRunId,
  expectSignedSafeInteger
} from './shared'
export const MAX_LLM_RETRY_DELAY_MS = 60_000
export const MAX_LLM_RETRY_ATTEMPTS = 6
export const LLM_RETRY_CATEGORIES = [
  'rate_limited',
  'quota_exhausted',
  'overloaded',
  'authentication',
  'invalid_request',
  'context_too_large',
  'network',
  'unknown'
] as const satisfies readonly AgentLlmRetryCategory[]

export function parseAgentToolDefinitions(value: unknown, context: string): AgentToolDefinition[] {
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

export function parseAgentStateSnapshot(value: unknown, context: string): AgentStateSnapshot {
  const item = expectRecord(value, context)
  expectOnlyKeys(item, ['status', 'activeRunId', 'lastError', 'updatedAt'] as const, context)
  return {
    status: expectEnum(
      item.status,
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

export function parseConversationTraceAttachments(
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

export function parseConversationTraceFolderReferences(
  value: unknown,
  context: string
): AgentFolderReference[] {
  if (value === undefined) return []
  return expectBoundedArray(value, context, 64).map((entry, index) => {
    const itemContext = `${context}[${index}]`
    const item = expectRecord(entry, itemContext)
    expectOnlyKeys(item, ['schemaVersion', 'id', 'name'] as const, itemContext)
    const schemaVersion = expectSafeInteger(item.schemaVersion, `${itemContext}.schemaVersion`, 0)
    if (schemaVersion !== 1) throw invalidProtocolValue(itemContext, 'unsupported schemaVersion')
    return {
      schemaVersion,
      id: expectOpaqueRunId(item.id, `${itemContext}.id`),
      name: expectBoundedString(item.name, `${itemContext}.name`, 4096)
    }
  })
}

export function parseAgentGuidanceEvent(
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
    expectOnlyKeys(record, [...commonKeys, 'folderReferences'] as const, context)
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
      ...(record.folderReferences === undefined
        ? {}
        : {
            folderReferences: parseConversationTraceFolderReferences(
              record.folderReferences,
              `${context}.folderReferences`
            )
          }),
      createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
    }
  }
  if (type === 'guidance_applied') {
    expectOnlyKeys(record, [...commonKeys, 'folderReferences', 'sequence'] as const, context)
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
      ...(record.folderReferences === undefined
        ? {}
        : {
            folderReferences: parseConversationTraceFolderReferences(
              record.folderReferences,
              `${context}.folderReferences`
            )
          }),
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

export function parseAgentToolCallForHost(value: unknown, context: string): AgentToolCall {
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

export function parseAgentToolCallEvent(
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

export function parseAgentToolResultForHost(value: unknown, context: string): AgentToolResult {
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

export function parseAgentTodoState(value: unknown, context: string): AgentTodoState {
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

export function parseActivatedSkillSummary(value: unknown, context: string): ActivatedSkillSummary {
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

export function parseAgentContextWindowSnapshot(
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

export function parseAgentUsage(value: unknown, context: string): AgentUsage {
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

export function parseAgentLlmRetryEvent(
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

export function parseAgentMessageStreamResetEvent(
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
