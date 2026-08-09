import type {
  AgentActionExecutionOutput,
  AgentChatOutput,
  AgentEvent,
  AgentLlmRetryCategory,
  AgentMcpArgumentSummary,
  AgentMcpInvocationDiagnostics,
  AgentMcpServerScope,
  AgentMcpToolApproval,
  AgentMcpToolApprovalSummary,
  AgentMcpToolInvocationEvent,
  AgentMcpToolInvocationIdentity,
  AgentMcpToolProvenance,
  AgentProposedAction,
  AgentToolCall,
  PendingAgentActionSnapshot
} from './agent'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'
import {
  isAgentCommandSessionEventType,
  parseAgentCommandSessionEvent
} from './agentCommandSessionParsers'

const DIGEST_PATTERN = /^[0-9a-f]{64}$/
const UUID_V4_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const SAFE_CODE_PATTERN = /^[a-zA-Z0-9_.-]{1,128}$/
const CANONICAL_PROVIDER_CODE_PATTERN = /^[a-z0-9][a-z0-9_.-]{0,127}$/
const MODEL_TOOL_CALL_ID_PATTERN = /^tc1_[a-zA-Z0-9_-]{43}$/
const MCP_APPROVAL_TTL_MS = 15 * 60 * 1000
const MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES = 1024 * 1024
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

/**
 * Parses Agent event families that have strict Host-boundary contracts. Legacy event families
 * retain their historical projection until they receive their own versioned parsers.
 */
export function parseAgentEventForHost(value: unknown): AgentEvent {
  const record = expectRecord(value, 'Agent event')
  if (record.type === 'llm_retry') {
    return parseAgentLlmRetryEvent(record)
  }
  if (record.type === 'message_stream_reset') {
    return parseAgentMessageStreamResetEvent(record)
  }
  if (isAgentCommandSessionEventType(record.type)) {
    return parseAgentCommandSessionEvent(record)
  }
  if (record.type === 'mcp_tool_invocation_state_changed') {
    expectOnlyKeys(record, ['type', 'runId', 'invocation'] as const, 'MCP Agent event')
    return {
      type: 'mcp_tool_invocation_state_changed',
      runId: expectOpaqueRunId(record.runId, 'MCP Agent event.runId'),
      invocation: parseAgentMcpToolInvocationEvent(record.invocation)
    }
  }

  if (record.type === 'approval_required') {
    const action = expectRecord(record.action, 'Agent approval event.action')
    if (action.type === 'mcp_tool_call') {
      expectOnlyKeys(record, ['type', 'runId', 'action'] as const, 'MCP approval event')
      const runId = expectOpaqueRunId(record.runId, 'MCP approval event.runId')
      const parsedAction = parseAgentMcpProposedAction(action)
      if (parsedAction.approval.identity.runId !== runId) {
        throw invalidProtocolValue(
          'MCP approval event',
          'action identity runId must match event runId'
        )
      }
      return { type: 'approval_required', runId, action: parsedAction }
    }
  }

  if (record.type === 'done' && Array.isArray(record.proposedActions)) {
    const hasMcpAction = record.proposedActions.some(
      (action) =>
        typeof action === 'object' &&
        action !== null &&
        !Array.isArray(action) &&
        'type' in action &&
        action.type === 'mcp_tool_call'
    )
    if (hasMcpAction) {
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
        'Agent done event containing MCP actions'
      )
      const runId = expectOpaqueRunId(record.runId, 'Agent done event containing MCP actions.runId')
      const proposedActions = parseMcpOnlyProposedActions(
        record.proposedActions,
        'Agent done event containing MCP actions.proposedActions',
        runId
      )
      return {
        type: 'done',
        runId,
        success: expectBoolean(record.success, 'Agent done event containing MCP actions.success'),
        ...(record.status === undefined
          ? {}
          : {
              status: expectEnum(
                record.status,
                [
                  'idle',
                  'queued',
                  'running',
                  'waiting_for_approval',
                  'completed',
                  'failed',
                  'cancelled'
                ] as const,
                'Agent done event containing MCP actions.status'
              )
            }),
        ...(record.content === undefined
          ? {}
          : {
              content: expectBoundedString(
                record.content,
                'Agent done event containing MCP actions.content',
                MAX_RENDERER_SAFE_AGENT_CONTENT_BYTES
              )
            }),
        ...(record.finishReason === undefined
          ? {}
          : {
              finishReason: expectBoundedString(
                record.finishReason,
                'Agent done event containing MCP actions.finishReason',
                256
              )
            }),
        proposedActions
      }
    }
  }

  return value as AgentEvent
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
      'maxAttempts',
      'reason'
    ] as const,
    context
  )
  if (record.reason !== undefined) {
    // Validate only for resource bounds. Provider-authored text is deliberately not returned.
    expectBoundedString(record.reason, `${context}.reason`, 16 * 1024)
  }
  const rawCategory =
    record.category === undefined
      ? undefined
      : expectBoundedNonEmptyString(record.category, `${context}.category`, 64)
  const category = LLM_RETRY_CATEGORIES.includes(rawCategory as AgentLlmRetryCategory)
    ? (rawCategory as AgentLlmRetryCategory)
    : 'unknown'
  const delayMs =
    record.delayMs === undefined ? 0 : expectSafeInteger(record.delayMs, `${context}.delayMs`, 0)
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
    retryAt:
      record.retryAt === undefined ? 0 : expectSafeInteger(record.retryAt, `${context}.retryAt`, 0),
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
    call.reason !== undefined
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
  const outcome =
    record.outcome === undefined
      ? undefined
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
    record.isError === undefined ? undefined : expectBoolean(record.isError, `${context}.isError`)
  const errorCode =
    record.errorCode === undefined
      ? undefined
      : expectSafeCode(record.errorCode, `${context}.errorCode`)
  const durationMs =
    record.durationMs === undefined
      ? undefined
      : expectSafeInteger(record.durationMs, `${context}.durationMs`, 0)
  const displayReason =
    record.displayReason === undefined
      ? undefined
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
    record.diagnostics === undefined
      ? undefined
      : parseAgentMcpInvocationDiagnostics(record.diagnostics, `${context}.diagnostics`)
  assertValidInvocationLifecycle(
    context,
    state,
    dispatchCertainty,
    outcome,
    isError,
    errorCode,
    durationMs,
    outputTruncated
  )
  if (diagnostics !== undefined) {
    assertValidInvocationDiagnostics(context, state, outcome, diagnostics)
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
    ...(displayReason === undefined ? {} : { displayReason }),
    external,
    state,
    dispatchCertainty,
    ...(outcome === undefined ? {} : { outcome }),
    ...(isError === undefined ? {} : { isError }),
    ...(errorCode === undefined ? {} : { errorCode }),
    ...(durationMs === undefined ? {} : { durationMs }),
    outputTruncated,
    ...(diagnostics === undefined ? {} : { diagnostics })
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
  outcome: AgentMcpToolInvocationEvent['outcome'],
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
  outcome: AgentMcpToolInvocationEvent['outcome'],
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
    if (action.type !== 'mcp_tool_call') {
      return entry as PendingAgentActionSnapshot
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
  if (record.actionType !== 'mcp_tool_call') {
    return value as AgentActionExecutionOutput
  }
  const context = 'MCP Agent action execution output'
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
    actionType: expectExactString(record.actionType, 'mcp_tool_call', `${context}.actionType`),
    toolName: expectBoundedNonEmptyString(record.toolName, `${context}.toolName`, 64),
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
  return {
    serverId: expectUuid(record.serverId, `${context}.serverId`),
    serverDisplayName: expectServerDisplayName(
      record.serverDisplayName,
      `${context}.serverDisplayName`
    ),
    scope: parseScope(record.scope, `${context}.scope`),
    rawToolName: expectDisplayText(record.rawToolName, `${context}.rawToolName`, 1024),
    modelToolName: expectDisplayText(record.modelToolName, `${context}.modelToolName`, 64),
    ...(record.displayReason === undefined
      ? {}
      : {
          displayReason: expectDisplayText(record.displayReason, `${context}.displayReason`, 512)
        }),
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
  const reason =
    record.reason === undefined
      ? undefined
      : expectDisplayText(record.reason, `${context}.reason`, 1024)
  return {
    id: expectModelToolCallId(record.id, `${context}.id`),
    tool: expectBoundedNonEmptyString(record.tool, `${context}.tool`, 64),
    args: {},
    approvalStatus: expectEnum(
      record.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      `${context}.approvalStatus`
    ),
    ...(reason === undefined ? {} : { reason })
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
    const isMcpApproval =
      eventRecord.type === 'approval_required' &&
      typeof eventRecord.action === 'object' &&
      eventRecord.action !== null &&
      !Array.isArray(eventRecord.action) &&
      'type' in eventRecord.action &&
      eventRecord.action.type === 'mcp_tool_call'
    const isMcpDone =
      eventRecord.type === 'done' &&
      Array.isArray(eventRecord.proposedActions) &&
      eventRecord.proposedActions.some(
        (action) =>
          typeof action === 'object' &&
          action !== null &&
          !Array.isArray(action) &&
          'type' in action &&
          action.type === 'mcp_tool_call'
      )
    if (eventRecord.type !== 'mcp_tool_invocation_state_changed' && !isMcpApproval && !isMcpDone) {
      return []
    }
    const parsedEvent = parseAgentEventForHost(eventRecord)
    if (parsedEvent.runId !== runId) {
      throw invalidProtocolValue(context, 'nested MCP event runId must match output runId')
    }
    return [parsedEvent]
  })
  const proposedActions = parseMcpOnlyProposedActions(
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
        'queued',
        'running',
        'waiting_for_approval',
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

function parseMcpOnlyProposedActions(
  value: unknown,
  context: string,
  enclosingRunId: string
): Extract<AgentProposedAction, { type: 'mcp_tool_call' }>[] {
  if (!Array.isArray(value) || value.length > MAX_RENDERER_SAFE_PROPOSED_ACTIONS) {
    throw invalidProtocolValue(
      context,
      `must be an array with at most ${MAX_RENDERER_SAFE_PROPOSED_ACTIONS} items`
    )
  }
  return value.map((action, index) => {
    const actionRecord = expectRecord(action, `${context}[${index}]`)
    if (actionRecord.type !== 'mcp_tool_call') {
      throw invalidProtocolValue(
        context,
        'mixed MCP and non-MCP proposed actions are not supported by this safe projection'
      )
    }
    const parsed = parseAgentMcpProposedAction(actionRecord)
    if (parsed.approval.identity.runId !== enclosingRunId) {
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
