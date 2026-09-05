import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from './skills/validation'

export const AGENT_START_MANUAL_CONTEXT_COMPACTION_METHOD = 'agent.startManualContextCompaction'
export const AGENT_GET_MANUAL_CONTEXT_COMPACTION_STATUS_METHOD =
  'agent.getManualContextCompactionStatus'
export const AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD = 'agent.cancelManualContextCompaction'
export const AGENT_MANUAL_CONTEXT_COMPACTION_NOTIFICATION_METHOD = 'agent.manualContextCompaction'

export interface AgentManualContextCompactionStartInput {
  conversationId: string
  requestId: string
}
export interface AgentManualContextCompactionStatusInput {
  conversationId: string
  operationId?: string
}
export interface AgentManualContextCompactionCancelInput {
  conversationId: string
  operationId: string
}
export interface AgentManualContextCompactionOperation {
  schemaVersion: 1
  operationId: string
  requestId: string
  conversationId: string
  status: 'running' | 'completed' | 'noop' | 'cancelled' | 'failed' | 'interrupted'
  /** Includes a cancelled request whose provider response is still draining. */
  isBusy?: boolean
  phase: 'preparing' | 'generating' | 'committing'
  startedAt: number
  updatedAt: number
  completedAt?: number
  modelId?: string
  summaryId?: string
  /** Presentation anchor only. Never a runtime cursor or usage owner. */
  coveredThroughMessageId?: string
  error?: string
}
export interface AgentManualContextCompactionStatusOutput {
  operations: AgentManualContextCompactionOperation[]
}
export type AgentManualContextCompactionNotification = AgentManualContextCompactionOperation

function boundedString(value: unknown, context: string, limit = 512): string {
  const result = expectString(value, context)
  if (!result.trim() || new TextEncoder().encode(result).byteLength > limit) {
    throw invalidProtocolValue(context, `expected a non-empty string of at most ${limit} bytes`)
  }
  return result
}

export function parseAgentManualContextCompactionStartInput(
  value: unknown
): AgentManualContextCompactionStartInput {
  const record = expectRecord(value, 'Manual compaction start')
  expectOnlyKeys(record, ['conversationId', 'requestId'], 'Manual compaction start')
  return {
    conversationId: boundedString(record.conversationId, 'conversationId'),
    requestId: boundedString(record.requestId, 'requestId')
  }
}

export function parseAgentManualContextCompactionStatusInput(
  value: unknown
): AgentManualContextCompactionStatusInput {
  const record = expectRecord(value, 'Manual compaction status')
  expectOnlyKeys(record, ['conversationId', 'operationId'], 'Manual compaction status')
  return {
    conversationId: boundedString(record.conversationId, 'conversationId'),
    ...(record.operationId === undefined
      ? {}
      : { operationId: boundedString(record.operationId, 'operationId') })
  }
}

export function parseAgentManualContextCompactionCancelInput(
  value: unknown
): AgentManualContextCompactionCancelInput {
  const result = parseAgentManualContextCompactionStatusInput(value)
  return { ...result, operationId: boundedString(result.operationId, 'operationId') }
}

export function parseAgentManualContextCompactionOperation(
  value: unknown
): AgentManualContextCompactionOperation {
  const context = 'Manual compaction operation'
  const record = expectRecord(value, context)
  expectSchemaVersion(record, 1, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'operationId',
      'requestId',
      'conversationId',
      'status',
      'phase',
      'isBusy',
      'startedAt',
      'updatedAt',
      'completedAt',
      'modelId',
      'summaryId',
      'coveredThroughMessageId',
      'error'
    ],
    context
  )
  const status = expectEnum(
    record.status,
    ['running', 'completed', 'noop', 'cancelled', 'failed', 'interrupted'] as const,
    'status'
  )
  const startedAt = expectSafeInteger(record.startedAt, 'startedAt', 0)
  const updatedAt = expectSafeInteger(record.updatedAt, 'updatedAt', startedAt)
  const completedAt =
    record.completedAt === undefined
      ? undefined
      : expectSafeInteger(record.completedAt, 'completedAt', startedAt)
  if ((status === 'running') !== (completedAt === undefined)) {
    throw invalidProtocolValue(context, 'only terminal operations must have completedAt')
  }
  if (status === 'completed' && (!record.summaryId || !record.coveredThroughMessageId)) {
    throw invalidProtocolValue(context, 'completed operations require summary and timeline anchor')
  }
  const optional: Partial<AgentManualContextCompactionOperation> = {}
  for (const key of ['modelId', 'summaryId', 'coveredThroughMessageId', 'error'] as const) {
    if (record[key] !== undefined)
      optional[key] = boundedString(record[key], key, key === 'error' ? 1024 : 512)
  }
  return {
    ...optional,
    ...(record.isBusy === undefined ? {} : { isBusy: expectBoolean(record.isBusy, 'isBusy') }),
    schemaVersion: 1,
    operationId: boundedString(record.operationId, 'operationId'),
    requestId: boundedString(record.requestId, 'requestId'),
    conversationId: boundedString(record.conversationId, 'conversationId'),
    status,
    phase: expectEnum(record.phase, ['preparing', 'generating', 'committing'] as const, 'phase'),
    startedAt,
    updatedAt,
    ...(completedAt === undefined ? {} : { completedAt })
  }
}

export function parseAgentManualContextCompactionStatusOutput(
  value: unknown
): AgentManualContextCompactionStatusOutput {
  const record = expectRecord(value, 'Manual compaction status output')
  expectOnlyKeys(record, ['operations'], 'Manual compaction status output')
  if (!Array.isArray(record.operations) || record.operations.length > 50) {
    throw invalidProtocolValue('operations', 'expected at most 50 operations')
  }
  const operations = record.operations.map(parseAgentManualContextCompactionOperation)
  if (new Set(operations.map((operation) => operation.operationId)).size !== operations.length) {
    throw invalidProtocolValue('operations', 'duplicate operation identity')
  }
  return { operations }
}
export const parseAgentManualContextCompactionNotification =
  parseAgentManualContextCompactionOperation
