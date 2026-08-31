import type { AgentMcpInvocationDiagnostics, AgentMcpToolInvocationEvent } from '../agent'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'
import {
  expectDisplayText,
  expectModelToolCallId,
  expectSafeCode,
  expectServerDisplayName,
  expectUuid,
  expectUuidV4
} from './shared'
export const MAX_MCP_DIAGNOSTIC_ARGUMENT_BYTES = 64 * 1024
export const MAX_MCP_DIAGNOSTIC_ARGUMENT_VALUES = 4096
export const MAX_MCP_DIAGNOSTIC_ARGUMENT_DEPTH = 32
export const MAX_MCP_DIAGNOSTIC_RESULT_BLOCKS = 128
export const MAX_MCP_DIAGNOSTIC_RESULT_BYTES = 4 * 1024 * 1024

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

export function parseAgentMcpInvocationDiagnostics(
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

export function parseAgentMcpResultSizeSummary(value: unknown, context: string) {
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

export function boundedDiagnosticInteger(value: unknown, context: string, maximum: number): number {
  const parsed = expectSafeInteger(value, context, 0)
  if (parsed > maximum) {
    throw invalidProtocolValue(context, `exceeded maximum ${maximum}`)
  }
  return parsed
}

export function assertValidInvocationDiagnostics(
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

export function assertValidInvocationLifecycle(
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
