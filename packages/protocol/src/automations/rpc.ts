import type {
  AutomationListInput,
  AutomationGetInput,
  AutomationCreateInput,
  AutomationUpdateInput,
  AutomationSetEnabledInput,
  AutomationRunNowInput,
  AutomationDeleteInput,
  AutomationRunsListInput,
  AutomationAttentionSummaryInput,
  AutomationAttentionAcknowledgeInput,
  AutomationListOutput,
  AutomationRunsListOutput,
  AutomationAttentionSummaryOutput,
  AutomationAttentionAcknowledgeOutput,
  AutomationDeleteOutput,
  AutomationOpenRequest,
  AutomationEvent,
  AutomationResync,
  AutomationErrorData
} from './types'
import { AUTOMATION_SCHEMA_VERSION } from './constants'
import {
  intInRange,
  LIST_MAX,
  boundedString,
  CURSOR_MAX,
  ID_MAX,
  nullableNonEmptyString,
  nullableInteger,
  SAFE_TEXT_MAX,
  nullableString
} from './validation'
import {
  expectRecord,
  expectOnlyKeys,
  expectSchemaVersion,
  expectEnum,
  expectSafeInteger,
  expectBoolean,
  expectArray,
  invalidProtocolValue
} from '../skills/validation'
import { parseConfig, parseAutomationTask } from './task'
import { parseAutomationRun, parseAutomationAttention } from './run'

export function parseAutomationListInput(value: unknown): AutomationListInput {
  const context = 'Automation list input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'status', 'query', 'cursor', 'limit'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const result: AutomationListInput = {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    limit: intInRange(record.limit, `${context}.limit`, 1, LIST_MAX)
  }
  if (record.status != null)
    result.status = expectEnum(record.status, ['active', 'paused'] as const, `${context}.status`)
  if (record.query != null)
    result.query = boundedString(record.query, `${context}.query`, 512, true)
  if (record.cursor != null)
    result.cursor = boundedString(record.cursor, `${context}.cursor`, CURSOR_MAX)
  return result
}

export function parseAutomationGetInput(value: unknown): AutomationGetInput {
  const context = 'Automation get input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX)
  }
}

export function parseAutomationCreateInput(value: unknown): AutomationCreateInput {
  const context = 'Automation create input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'requestId',
      'status',
      'title',
      'prompt',
      'destination',
      'permissionMode',
      'permissionModeVersion',
      'schedule',
      'notificationPolicy'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    requestId: boundedString(record.requestId, `${context}.requestId`, ID_MAX),
    status: expectEnum(record.status, ['active', 'paused'] as const, `${context}.status`),
    ...parseConfig(record, context)
  }
}

export function parseAutomationUpdateInput(value: unknown): AutomationUpdateInput {
  const context = 'Automation update input'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'automationId',
      'expectedRevision',
      'title',
      'prompt',
      'destination',
      'permissionMode',
      'permissionModeVersion',
      'schedule',
      'notificationPolicy'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context}.expectedRevision`, 0),
    ...parseConfig(record, context)
  }
}

export function parseAutomationSetEnabledInput(value: unknown): AutomationSetEnabledInput {
  const context = 'Automation set enabled input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'expectedRevision', 'enabled'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context}.expectedRevision`, 0),
    enabled: expectBoolean(record.enabled, `${context}.enabled`)
  }
}

export function parseAutomationRunNowInput(value: unknown): AutomationRunNowInput {
  const context = 'Automation run now input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'requestId'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    requestId: boundedString(record.requestId, `${context}.requestId`, ID_MAX)
  }
}

export function parseAutomationDeleteInput(value: unknown): AutomationDeleteInput {
  const context = 'Automation delete input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'expectedRevision'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    expectedRevision: expectSafeInteger(record.expectedRevision, `${context}.expectedRevision`, 0)
  }
}

export function parseAutomationRunsListInput(value: unknown): AutomationRunsListInput {
  const context = 'Automation runs list input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'cursor', 'limit'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const result: AutomationRunsListInput = {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    limit: intInRange(record.limit, `${context}.limit`, 1, LIST_MAX)
  }
  if (record.cursor != null)
    result.cursor = boundedString(record.cursor, `${context}.cursor`, CURSOR_MAX)
  return result
}

export function parseAutomationAttentionSummaryInput(
  value: unknown
): AutomationAttentionSummaryInput {
  const context = 'Automation attention summary input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'cursor', 'limit'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const result: AutomationAttentionSummaryInput = {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    limit: intInRange(record.limit, `${context}.limit`, 1, LIST_MAX)
  }
  if (record.cursor != null)
    result.cursor = boundedString(record.cursor, `${context}.cursor`, CURSOR_MAX)
  return result
}

export function parseAutomationAttentionAcknowledgeInput(
  value: unknown
): AutomationAttentionAcknowledgeInput {
  const context = 'Automation attention acknowledge input'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'attentionId'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    attentionId: boundedString(record.attentionId, `${context}.attentionId`, ID_MAX)
  }
}

export function parseAutomationListOutput(value: unknown): AutomationListOutput {
  const context = 'Automation list output'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'tasks', 'nextCursor', 'counts', 'attentionCount', 'lastSequence'],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const counts = expectRecord(record.counts, `${context}.counts`)
  expectOnlyKeys(counts, ['all', 'active', 'paused'], `${context}.counts`)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    tasks: expectArray(record.tasks, `${context}.tasks`).map(parseAutomationTask),
    nextCursor: nullableNonEmptyString(record.nextCursor, `${context}.nextCursor`, CURSOR_MAX),
    counts: {
      all: expectSafeInteger(counts.all, `${context}.counts.all`, 0),
      active: expectSafeInteger(counts.active, `${context}.counts.active`, 0),
      paused: expectSafeInteger(counts.paused, `${context}.counts.paused`, 0)
    },
    attentionCount: expectSafeInteger(record.attentionCount, `${context}.attentionCount`, 0),
    lastSequence: expectSafeInteger(record.lastSequence, `${context}.lastSequence`, 0)
  }
}

export function parseAutomationRunsListOutput(value: unknown): AutomationRunsListOutput {
  const context = 'Automation runs list output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'runs', 'nextCursor'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const automationId = boundedString(record.automationId, `${context}.automationId`, ID_MAX)
  const runs = expectArray(record.runs, `${context}.runs`).map(parseAutomationRun)
  if (runs.some((run) => run.automationId !== automationId))
    throw invalidProtocolValue(`${context}.runs`, 'all runs must match automationId')
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId,
    runs,
    nextCursor: nullableNonEmptyString(record.nextCursor, `${context}.nextCursor`, CURSOR_MAX)
  }
}

export function parseAutomationAttentionSummaryOutput(
  value: unknown
): AutomationAttentionSummaryOutput {
  const context = 'Automation attention summary output'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'unreadCount', 'items', 'nextCursor', 'lastSequence'],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    unreadCount: expectSafeInteger(record.unreadCount, `${context}.unreadCount`, 0),
    items: expectArray(record.items, `${context}.items`).map(parseAutomationAttention),
    nextCursor: nullableNonEmptyString(record.nextCursor, `${context}.nextCursor`, CURSOR_MAX),
    lastSequence: expectSafeInteger(record.lastSequence, `${context}.lastSequence`, 0)
  }
}

export function parseAutomationAttentionAcknowledgeOutput(
  value: unknown
): AutomationAttentionAcknowledgeOutput {
  const context = 'Automation attention acknowledge output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'attention'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    attention: parseAutomationAttention(record.attention)
  }
}

export function parseAutomationDeleteOutput(value: unknown): AutomationDeleteOutput {
  const context = 'Automation delete output'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'deletedAt'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    deletedAt: expectSafeInteger(record.deletedAt, `${context}.deletedAt`, 0)
  }
}

export function parseAutomationOpenRequest(value: unknown): AutomationOpenRequest {
  const context = 'Automation open request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'automationId', 'runId', 'destination'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const destination = expectRecord(record.destination, `${context}.destination`)
  const kind = expectEnum(
    destination.kind,
    ['task', 'conversation'] as const,
    `${context}.destination.kind`
  )
  if (kind === 'task') {
    expectOnlyKeys(destination, ['kind'], `${context}.destination`)
    return {
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
      runId: nullableNonEmptyString(record.runId, `${context}.runId`, ID_MAX),
      destination: { kind }
    }
  }
  expectOnlyKeys(destination, ['kind', 'conversationId', 'messageId'], `${context}.destination`)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    runId: nullableNonEmptyString(record.runId, `${context}.runId`, ID_MAX),
    destination: {
      kind,
      conversationId: boundedString(
        destination.conversationId,
        `${context}.destination.conversationId`,
        ID_MAX
      ),
      messageId: nullableNonEmptyString(
        destination.messageId,
        `${context}.destination.messageId`,
        ID_MAX
      )
    }
  }
}

export function parseAutomationEvent(value: unknown): AutomationEvent {
  const context = 'Automation event'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'sequence',
      'eventId',
      'kind',
      'automationId',
      'runId',
      'resourceRevision',
      'occurredAt'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    sequence: expectSafeInteger(record.sequence, `${context}.sequence`, 0),
    eventId: boundedString(record.eventId, `${context}.eventId`, ID_MAX),
    kind: expectEnum(
      record.kind,
      [
        'created',
        'updated',
        'deleted',
        'run_updated',
        'attention_changed',
        'notification_requested'
      ] as const,
      `${context}.kind`
    ),
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    runId: nullableNonEmptyString(record.runId, `${context}.runId`, ID_MAX),
    resourceRevision: nullableInteger(record.resourceRevision, `${context}.resourceRevision`),
    occurredAt: expectSafeInteger(record.occurredAt, `${context}.occurredAt`, 0)
  }
}

export function parseAutomationResync(value: unknown): AutomationResync {
  const context = 'Automation resync'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'reason', 'lastSequence', 'occurredAt'], context)
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  if (record.reason !== 'core_started')
    throw invalidProtocolValue(`${context}.reason`, 'must be core_started')
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    reason: 'core_started',
    lastSequence: expectSafeInteger(record.lastSequence, `${context}.lastSequence`, 0),
    occurredAt: expectSafeInteger(record.occurredAt, `${context}.occurredAt`, 0)
  }
}

export function parseAutomationErrorData(value: unknown): AutomationErrorData {
  const context = 'Automation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'type',
      'code',
      'message',
      'automationId',
      'currentRevision',
      'field',
      'retryable'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  if (record.type !== 'automation')
    throw invalidProtocolValue(`${context}.type`, 'must be automation')
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    type: 'automation',
    code: expectEnum(
      record.code,
      [
        'not_found',
        'revision_conflict',
        'validation',
        'run_already_active',
        'target_invalid',
        'permission_disabled',
        'schedule_invalid',
        'internal'
      ] as const,
      `${context}.code`
    ),
    message: boundedString(record.message, `${context}.message`, SAFE_TEXT_MAX),
    automationId: nullableNonEmptyString(record.automationId, `${context}.automationId`, ID_MAX),
    currentRevision: nullableInteger(record.currentRevision, `${context}.currentRevision`),
    field: nullableString(record.field, `${context}.field`, 256),
    retryable: expectBoolean(record.retryable, `${context}.retryable`)
  }
}
