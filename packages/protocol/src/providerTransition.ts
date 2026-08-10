import {
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from './skills/validation'

/** Provider-transition JSON-RPC names are a versioned Host/Core transport contract. */
export const AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD = 'agent.preflightProviderTransition'
export const AGENT_START_PROVIDER_TRANSITION_METHOD = 'agent.startProviderTransition'
export const AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD = 'agent.getProviderTransitionStatus'
export const AGENT_PROVIDER_TRANSITION_NOTIFICATION_METHOD = 'agent.providerTransition'

export const AGENT_PROVIDER_TRANSITION_SCHEMA_VERSION = 1 as const

const MAX_ID_BYTES = 512
const MAX_TRANSITION_TOKEN_BYTES = 4096
const MAX_SAFE_MESSAGE_BYTES = 1024
const MAX_SAFE_ERROR_CODE_BYTES = 128
const MAX_STATUS_OPERATIONS = 50

const PROVIDER_TRANSITION_ERROR_CODES = [
  'provider_transition_stale',
  'provider_transition_generation_failed',
  'provider_transition_commit_failed',
  'provider_transition_interrupted'
] as const

export type AgentProviderTransitionDecision = 'compatible' | 'requires_compaction' | 'blocked'

/** Renderer-safe classification only. It deliberately exposes no runtime capability or key. */
export type AgentProviderTransitionReason =
  | 'same_protocol'
  | 'no_incompatible_history'
  | 'api_provider_changed'
  | 'provider_protocol_changed'
  | 'active_run'
  | 'pending_approval'
  | 'unsupported_target'

export interface AgentProviderTransitionPreflightInput {
  conversationId: string
  targetModelId: string
}

export type AgentProviderTransitionPreflightOutput = {
  conversationId: string
  targetModelId: string
} & (
  | {
      decision: 'compatible'
      reason: 'same_protocol' | 'no_incompatible_history'
      /** Safe operation identity derived from the same Host authority as transitionToken. */
      operationId: string
      transitionToken: string
      message?: string
    }
  | {
      decision: 'requires_compaction'
      reason: 'api_provider_changed' | 'provider_protocol_changed'
      /** Safe operation identity derived from the same Host authority as transitionToken. */
      operationId: string
      transitionToken: string
      message?: string
    }
  | {
      decision: 'blocked'
      reason: 'active_run' | 'pending_approval' | 'unsupported_target'
      message?: string
    }
)

export interface AgentProviderTransitionStartInput {
  conversationId: string
  targetModelId: string
  /** Opaque, short-lived Host authority bound to the preflight history and target model. */
  transitionToken: string
}

export interface AgentProviderTransitionStatusInput {
  conversationId: string
  operationId?: string
}

export type AgentProviderTransitionRecovery = 'retry'
export type AgentProviderTransitionErrorCode = (typeof PROVIDER_TRANSITION_ERROR_CODES)[number]

export interface AgentProviderTransitionError {
  code: AgentProviderTransitionErrorCode
  message: string
  recovery: AgentProviderTransitionRecovery
}

interface AgentProviderTransitionOperationBase {
  schemaVersion: typeof AGENT_PROVIDER_TRANSITION_SCHEMA_VERSION
  operationId: string
  conversationId: string
  targetModelId: string
  /** Safe Timeline anchor only; never a Provider cursor or continuation reference. */
  coveredThroughMessageId?: string
  startedAt: number
}

export type AgentProviderTransitionOperation =
  | (AgentProviderTransitionOperationBase & {
      status: 'running'
    })
  | (AgentProviderTransitionOperationBase & {
      status: 'completed'
      /** Authoritative model committed by Host. Must equal targetModelId. */
      modelId: string
      /** Present when the transition established a compaction boundary. */
      summaryId?: string
      completedAt: number
      /** Authoritative conversation.updated_at committed in the same Host transaction. */
      conversationUpdatedAt: number
    })
  | (AgentProviderTransitionOperationBase & {
      status: 'failed'
      error: AgentProviderTransitionError
      completedAt: number
    })

export interface AgentProviderTransitionStatusOutput {
  operations: AgentProviderTransitionOperation[]
}

export type AgentProviderTransitionNotification = AgentProviderTransitionOperation

export function parseAgentProviderTransitionPreflightInput(
  value: unknown
): AgentProviderTransitionPreflightInput {
  const context = 'Provider transition preflight input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['conversationId', 'targetModelId'] as const, context)
  return {
    conversationId: expectBoundedNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      MAX_ID_BYTES
    ),
    targetModelId: expectBoundedNonEmptyString(
      record.targetModelId,
      `${context}.targetModelId`,
      MAX_ID_BYTES
    )
  }
}

export function parseAgentProviderTransitionPreflightOutput(
  value: unknown
): AgentProviderTransitionPreflightOutput {
  const context = 'Provider transition preflight output'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'conversationId',
      'targetModelId',
      'decision',
      'reason',
      'operationId',
      'transitionToken',
      'message'
    ] as const,
    context
  )
  const common = {
    conversationId: expectBoundedNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      MAX_ID_BYTES
    ),
    targetModelId: expectBoundedNonEmptyString(
      record.targetModelId,
      `${context}.targetModelId`,
      MAX_ID_BYTES
    )
  }
  const decision = expectEnum(
    record.decision,
    ['compatible', 'requires_compaction', 'blocked'] as const,
    `${context}.decision`
  )
  const reason = expectEnum(
    record.reason,
    [
      'same_protocol',
      'no_incompatible_history',
      'api_provider_changed',
      'provider_protocol_changed',
      'active_run',
      'pending_approval',
      'unsupported_target'
    ] as const,
    `${context}.reason`
  )
  const message = parseOptionalBoundedString(
    record.message,
    `${context}.message`,
    MAX_SAFE_MESSAGE_BYTES
  )

  if (decision === 'compatible') {
    if (reason !== 'same_protocol' && reason !== 'no_incompatible_history') {
      throw invalidProtocolValue(context, `reason ${reason} is invalid for compatible`)
    }
    return {
      ...common,
      decision,
      reason,
      operationId: expectBoundedNonEmptyString(
        record.operationId,
        `${context}.operationId`,
        MAX_ID_BYTES
      ),
      transitionToken: expectBoundedNonEmptyString(
        record.transitionToken,
        `${context}.transitionToken`,
        MAX_TRANSITION_TOKEN_BYTES
      ),
      ...(message === undefined ? {} : { message })
    }
  }

  if (decision === 'requires_compaction') {
    if (reason !== 'api_provider_changed' && reason !== 'provider_protocol_changed') {
      throw invalidProtocolValue(context, `reason ${reason} is invalid for requires_compaction`)
    }
    return {
      ...common,
      decision,
      reason,
      operationId: expectBoundedNonEmptyString(
        record.operationId,
        `${context}.operationId`,
        MAX_ID_BYTES
      ),
      transitionToken: expectBoundedNonEmptyString(
        record.transitionToken,
        `${context}.transitionToken`,
        MAX_TRANSITION_TOKEN_BYTES
      ),
      ...(message === undefined ? {} : { message })
    }
  }

  if (reason !== 'active_run' && reason !== 'pending_approval' && reason !== 'unsupported_target') {
    throw invalidProtocolValue(context, `reason ${reason} is invalid for blocked`)
  }
  if (record.transitionToken !== undefined) {
    throw invalidProtocolValue(context, 'blocked output must not carry transitionToken')
  }
  if (record.operationId !== undefined) {
    throw invalidProtocolValue(context, 'blocked output must not carry operationId')
  }
  return {
    ...common,
    decision,
    reason,
    ...(message === undefined ? {} : { message })
  }
}

export function parseAgentProviderTransitionStartInput(
  value: unknown
): AgentProviderTransitionStartInput {
  const context = 'Provider transition start input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['conversationId', 'targetModelId', 'transitionToken'] as const, context)
  return {
    conversationId: expectBoundedNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      MAX_ID_BYTES
    ),
    targetModelId: expectBoundedNonEmptyString(
      record.targetModelId,
      `${context}.targetModelId`,
      MAX_ID_BYTES
    ),
    transitionToken: expectBoundedNonEmptyString(
      record.transitionToken,
      `${context}.transitionToken`,
      MAX_TRANSITION_TOKEN_BYTES
    )
  }
}

export function parseAgentProviderTransitionStatusInput(
  value: unknown
): AgentProviderTransitionStatusInput {
  const context = 'Provider transition status input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['conversationId', 'operationId'] as const, context)
  return {
    conversationId: expectBoundedNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      MAX_ID_BYTES
    ),
    ...(record.operationId === undefined
      ? {}
      : {
          operationId: expectBoundedNonEmptyString(
            record.operationId,
            `${context}.operationId`,
            MAX_ID_BYTES
          )
        })
  }
}

export function parseAgentProviderTransitionOperation(
  value: unknown
): AgentProviderTransitionOperation {
  const context = 'Provider transition operation'
  const record = expectRecord(value, context)
  expectSchemaVersion(record, AGENT_PROVIDER_TRANSITION_SCHEMA_VERSION, context)
  const status = expectEnum(
    record.status,
    ['running', 'completed', 'failed'] as const,
    `${context}.status`
  )
  const allowedKeys = [
    'schemaVersion',
    'operationId',
    'conversationId',
    'targetModelId',
    'coveredThroughMessageId',
    'status',
    'startedAt',
    ...(status === 'completed'
      ? ['modelId', 'summaryId', 'completedAt', 'conversationUpdatedAt']
      : []),
    ...(status === 'failed' ? ['error', 'completedAt'] : [])
  ]
  expectOnlyKeys(record, allowedKeys, context)

  const common: AgentProviderTransitionOperationBase = {
    schemaVersion: AGENT_PROVIDER_TRANSITION_SCHEMA_VERSION,
    operationId: expectBoundedNonEmptyString(
      record.operationId,
      `${context}.operationId`,
      MAX_ID_BYTES
    ),
    conversationId: expectBoundedNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      MAX_ID_BYTES
    ),
    targetModelId: expectBoundedNonEmptyString(
      record.targetModelId,
      `${context}.targetModelId`,
      MAX_ID_BYTES
    ),
    ...(record.coveredThroughMessageId === undefined
      ? {}
      : {
          coveredThroughMessageId: expectBoundedNonEmptyString(
            record.coveredThroughMessageId,
            `${context}.coveredThroughMessageId`,
            MAX_ID_BYTES
          )
        }),
    startedAt: expectSafeInteger(record.startedAt, `${context}.startedAt`, 0)
  }

  if (status === 'running') return { ...common, status }

  const completedAt = expectSafeInteger(record.completedAt, `${context}.completedAt`, 0)
  if (completedAt < common.startedAt) {
    throw invalidProtocolValue(context, 'completedAt must not precede startedAt')
  }

  if (status === 'completed') {
    const modelId = expectBoundedNonEmptyString(record.modelId, `${context}.modelId`, MAX_ID_BYTES)
    if (modelId !== common.targetModelId) {
      throw invalidProtocolValue(context, 'modelId must equal targetModelId')
    }
    const summaryId = parseOptionalBoundedNonEmptyString(
      record.summaryId,
      `${context}.summaryId`,
      MAX_ID_BYTES
    )
    if ((summaryId === undefined) !== (common.coveredThroughMessageId === undefined)) {
      throw invalidProtocolValue(
        context,
        'summaryId and coveredThroughMessageId must either both be present or both be absent'
      )
    }
    return {
      ...common,
      status,
      modelId,
      ...(summaryId === undefined ? {} : { summaryId }),
      completedAt,
      conversationUpdatedAt: expectSafeInteger(
        record.conversationUpdatedAt,
        `${context}.conversationUpdatedAt`,
        0
      )
    }
  }

  return {
    ...common,
    status,
    error: parseAgentProviderTransitionError(record.error),
    completedAt
  }
}

export function parseAgentProviderTransitionStatusOutput(
  value: unknown
): AgentProviderTransitionStatusOutput {
  const context = 'Provider transition status output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['operations'] as const, context)
  if (!Array.isArray(record.operations) || record.operations.length > MAX_STATUS_OPERATIONS) {
    throw invalidProtocolValue(
      context,
      `operations must be an array with at most ${MAX_STATUS_OPERATIONS} items`
    )
  }
  const operations = record.operations.map(parseAgentProviderTransitionOperation)
  if (new Set(operations.map((operation) => operation.operationId)).size !== operations.length) {
    throw invalidProtocolValue(context, 'operationId values must be unique')
  }
  if (
    operations.some(
      (operation, index) => index > 0 && operations[index - 1]!.startedAt < operation.startedAt
    )
  ) {
    throw invalidProtocolValue(context, 'operations must be ordered by startedAt descending')
  }
  return { operations }
}

export const parseAgentProviderTransitionNotification = parseAgentProviderTransitionOperation

function parseAgentProviderTransitionError(value: unknown): AgentProviderTransitionError {
  const context = 'Provider transition error'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['code', 'message', 'recovery'] as const, context)
  return {
    code: expectEnum(
      expectBoundedNonEmptyString(record.code, `${context}.code`, MAX_SAFE_ERROR_CODE_BYTES),
      PROVIDER_TRANSITION_ERROR_CODES,
      `${context}.code`
    ),
    message: expectBoundedNonEmptyString(
      record.message,
      `${context}.message`,
      MAX_SAFE_MESSAGE_BYTES
    ),
    recovery: expectEnum(record.recovery, ['retry'] as const, `${context}.recovery`)
  }
}

function parseOptionalBoundedString(
  value: unknown,
  context: string,
  maxBytes: number
): string | undefined {
  if (value === undefined) return undefined
  const stringValue = expectString(value, context)
  if (new TextEncoder().encode(stringValue).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `must not exceed ${maxBytes} UTF-8 bytes`)
  }
  return stringValue
}

function parseOptionalBoundedNonEmptyString(
  value: unknown,
  context: string,
  maxBytes: number
): string | undefined {
  if (value === undefined) return undefined
  return expectBoundedNonEmptyString(value, context, maxBytes)
}

function expectBoundedNonEmptyString(value: unknown, context: string, maxBytes: number): string {
  const stringValue = expectString(value, context)
  if (stringValue.trim().length === 0) {
    throw invalidProtocolValue(context, 'expected a non-empty string')
  }
  if (new TextEncoder().encode(stringValue).byteLength > maxBytes) {
    throw invalidProtocolValue(context, `must not exceed ${maxBytes} UTF-8 bytes`)
  }
  return stringValue
}
