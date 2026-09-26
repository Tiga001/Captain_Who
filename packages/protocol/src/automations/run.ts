import type { AutomationAttention, AutomationRun } from './types'
import { AUTOMATION_SCHEMA_VERSION } from './constants'
import {
  boundedString,
  ID_MAX,
  nullableNonEmptyString,
  SAFE_TEXT_MAX,
  nullableInteger,
  nullableString
} from './validation'
import {
  expectRecord,
  expectOnlyKeys,
  expectSchemaVersion,
  expectEnum,
  expectSafeInteger,
  invalidProtocolValue
} from '../skills/validation'

export function parseAutomationAttention(value: unknown): AutomationAttention {
  const context = 'Automation attention'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'attentionId',
      'automationId',
      'runId',
      'kind',
      'message',
      'createdAt',
      'readAt'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    attentionId: boundedString(record.attentionId, `${context}.attentionId`, ID_MAX),
    automationId: boundedString(record.automationId, `${context}.automationId`, ID_MAX),
    runId: nullableNonEmptyString(record.runId, `${context}.runId`, ID_MAX),
    kind: expectEnum(
      record.kind,
      ['waiting_for_approval', 'run_failed', 'important_update', 'configuration_blocked'] as const,
      `${context}.kind`
    ),
    message: boundedString(record.message, `${context}.message`, SAFE_TEXT_MAX, false, true),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    readAt: nullableInteger(record.readAt, `${context}.readAt`)
  }
}

export function parseAutomationRun(value: unknown): AutomationRun {
  const context = 'Automation run'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'runId',
      'automationId',
      'configRevision',
      'triggerKind',
      'scheduledFor',
      'status',
      'conversationId',
      'userMessageId',
      'assistantMessageId',
      'reportKind',
      'resultPreview',
      'errorCode',
      'errorMessage',
      'attention',
      'createdAt',
      'startedAt',
      'completedAt',
      'updatedAt'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const runId = boundedString(record.runId, `${context}.runId`, ID_MAX)
  const automationId = boundedString(record.automationId, `${context}.automationId`, ID_MAX)
  const attention = record.attention === null ? null : parseAutomationAttention(record.attention)
  if (attention !== null && attention.automationId !== automationId)
    throw invalidProtocolValue(`${context}.attention.automationId`, 'must match automationId')
  if (attention !== null && attention.runId !== runId)
    throw invalidProtocolValue(`${context}.attention.runId`, 'must match runId')
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    runId,
    automationId,
    configRevision: expectSafeInteger(record.configRevision, `${context}.configRevision`, 0),
    triggerKind: expectEnum(
      record.triggerKind,
      ['scheduled', 'manual', 'recovery'] as const,
      `${context}.triggerKind`
    ),
    scheduledFor: nullableInteger(record.scheduledFor, `${context}.scheduledFor`),
    status: expectEnum(
      record.status,
      [
        'queued',
        'starting',
        'running',
        'waiting_for_approval',
        'completed',
        'failed',
        'cancelled'
      ] as const,
      `${context}.status`
    ),
    conversationId: nullableNonEmptyString(
      record.conversationId,
      `${context}.conversationId`,
      ID_MAX
    ),
    userMessageId: nullableNonEmptyString(record.userMessageId, `${context}.userMessageId`, ID_MAX),
    assistantMessageId: nullableNonEmptyString(
      record.assistantMessageId,
      `${context}.assistantMessageId`,
      ID_MAX
    ),
    reportKind: expectEnum(
      record.reportKind,
      ['no_change', 'important_update', 'completed', 'unknown'] as const,
      `${context}.reportKind`
    ),
    resultPreview: nullableString(
      record.resultPreview,
      `${context}.resultPreview`,
      SAFE_TEXT_MAX,
      true
    ),
    errorCode: nullableString(record.errorCode, `${context}.errorCode`, 128),
    errorMessage: nullableString(
      record.errorMessage,
      `${context}.errorMessage`,
      SAFE_TEXT_MAX,
      true
    ),
    attention,
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    startedAt: nullableInteger(record.startedAt, `${context}.startedAt`),
    completedAt: nullableInteger(record.completedAt, `${context}.completedAt`),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
}
