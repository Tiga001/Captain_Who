import type { AgentPermissions } from './agent'
import type { ProviderReasoningEffort, ProviderReasoningMode } from './storage'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue
} from './skills/validation'

/** Stable JSON-RPC contract shared by Renderer, Electron Main, and Core. */
export const AUTOMATION_SCHEMA_VERSION = 1 as const
export const AUTOMATION_PERMISSION_MODE_VERSION = 2 as const
export const AUTOMATION_ERROR_CODE = -32045 as const

export const AUTOMATION_LIST_METHOD = 'automation.list'
export const AUTOMATION_GET_METHOD = 'automation.get'
export const AUTOMATION_CREATE_METHOD = 'automation.create'
export const AUTOMATION_UPDATE_METHOD = 'automation.update'
export const AUTOMATION_SET_ENABLED_METHOD = 'automation.setEnabled'
export const AUTOMATION_RUN_NOW_METHOD = 'automation.runNow'
export const AUTOMATION_DELETE_METHOD = 'automation.delete'
export const AUTOMATION_RUNS_LIST_METHOD = 'automation.runs.list'
export const AUTOMATION_ATTENTION_SUMMARY_METHOD = 'automation.attention.summary'
export const AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD = 'automation.attention.acknowledge'
export const AUTOMATION_EVENT_NOTIFICATION_METHOD = 'automation.event'
export const AUTOMATION_RESYNC_NOTIFICATION_METHOD = 'automation.resync'

export type AutomationStatus = 'active' | 'paused'
export type AutomationPermissionMode = 'default' | 'full' | 'custom'
export type AutomationNotificationPolicy = 'all_runs' | 'unsuccessful_only' | 'important_updates'
export type AutomationWeekday =
  'monday' | 'tuesday' | 'wednesday' | 'thursday' | 'friday' | 'saturday' | 'sunday'
export type AutomationIntervalUnit = 'minutes' | 'hours' | 'days'
export type AutomationCustomFrequency = 'hourly' | 'daily' | 'weekly' | 'monthly' | 'yearly'

export type AutomationScheduleInput =
  | {
      kind: 'interval'
      amount: number
      unit: AutomationIntervalUnit
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'daily' | 'weekdays'
      timeMinutes: number
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'weekly'
      weekdays: AutomationWeekday[]
      timeMinutes: number
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'custom'
      frequency: 'hourly'
      interval: number
      minuteOfHour: number
      timeMinutes?: never
      weekdays?: never
      monthDays?: never
      months?: never
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'custom'
      frequency: 'daily'
      interval: number
      minuteOfHour?: never
      timeMinutes: number
      weekdays?: never
      monthDays?: never
      months?: never
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'custom'
      frequency: 'weekly'
      interval: number
      minuteOfHour?: never
      timeMinutes: number
      weekdays: AutomationWeekday[]
      monthDays?: never
      months?: never
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'custom'
      frequency: 'monthly'
      interval: number
      minuteOfHour?: never
      timeMinutes: number
      weekdays?: never
      monthDays: number[]
      months?: never
      anchorAt: number
      timezone: string
    }
  | {
      kind: 'custom'
      frequency: 'yearly'
      interval: number
      minuteOfHour?: never
      timeMinutes: number
      weekdays?: never
      monthDays: number[]
      months: number[]
      anchorAt: number
      timezone: string
    }

export type AutomationSchedule = AutomationScheduleInput

export interface AutomationReasoningProjection {
  source: 'model_config'
  mode: ProviderReasoningMode
  effort: ProviderReasoningEffort
}

export type AutomationDestinationInput =
  | {
      kind: 'new_chat'
      projectBinding: 'none' | 'project'
      projectId: string | null
      modelId: string
    }
  | { kind: 'existing_chat'; conversationId: string }

export type AutomationDestination =
  | (Extract<AutomationDestinationInput, { kind: 'new_chat' }> & {
      reasoning: AutomationReasoningProjection
    })
  | Extract<AutomationDestinationInput, { kind: 'existing_chat' }>

export type AutomationHealth =
  | { state: 'ok' }
  | {
      state: 'blocked'
      code:
        | 'target_missing'
        | 'target_archived'
        | 'target_invalid'
        | 'project_missing'
        | 'project_path_missing'
        | 'model_missing'
        | 'model_disabled'
        | 'model_unavailable'
        | 'permission_disabled'
        | 'configuration_invalid'
        | 'schedule_invalid'
      message: string
    }

export type AutomationRunStatus =
  'queued' | 'starting' | 'running' | 'waiting_for_approval' | 'completed' | 'failed' | 'cancelled'
export type AutomationRunTriggerKind = 'scheduled' | 'manual' | 'recovery'
export type AutomationReportKind = 'no_change' | 'important_update' | 'completed' | 'unknown'
export type AutomationAttentionKind =
  'waiting_for_approval' | 'run_failed' | 'important_update' | 'configuration_blocked'

export interface AutomationAttention {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  attentionId: string
  automationId: string
  runId: string | null
  kind: AutomationAttentionKind
  message: string
  createdAt: number
  readAt: number | null
}

export interface AutomationRun {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  runId: string
  automationId: string
  configRevision: number
  triggerKind: AutomationRunTriggerKind
  scheduledFor: number | null
  status: AutomationRunStatus
  conversationId: string | null
  userMessageId: string | null
  assistantMessageId: string | null
  reportKind: AutomationReportKind
  resultPreview: string | null
  errorCode: string | null
  errorMessage: string | null
  attention: AutomationAttention | null
  createdAt: number
  startedAt: number | null
  completedAt: number | null
  updatedAt: number
}

export interface AutomationTargetSnapshot {
  projectName: string | null
  conversationTitle: string | null
  modelDisplayName: string | null
}

export interface AutomationTask {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  title: string
  prompt: string
  status: AutomationStatus
  health: AutomationHealth
  destination: AutomationDestination
  permissionMode: AutomationPermissionMode
  permissionModeVersion: typeof AUTOMATION_PERMISSION_MODE_VERSION
  resolvedPermissions: AgentPermissions
  schedule: AutomationSchedule
  scheduleSummary: string
  rrule: string
  timezone: string
  notificationPolicy: AutomationNotificationPolicy
  targetSnapshot: AutomationTargetSnapshot
  nextRunAt: number | null
  lastScheduledAt: number | null
  lastRunAt: number | null
  latestRun: AutomationRun | null
  attention: AutomationAttention | null
  revision: number
  createdAt: number
  updatedAt: number
}

/** List rows deliberately use the same authoritative shape as detail in v1. */
export type AutomationTaskSummary = AutomationTask

export interface AutomationMutableConfigInput {
  title: string
  prompt: string
  destination: AutomationDestinationInput
  permissionMode: AutomationPermissionMode
  permissionModeVersion: typeof AUTOMATION_PERMISSION_MODE_VERSION
  schedule: AutomationScheduleInput
  notificationPolicy: AutomationNotificationPolicy
}

export interface AutomationListInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  status?: AutomationStatus
  query?: string
  cursor?: string
  limit: number
}
export interface AutomationGetInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
}
export interface AutomationCreateInput extends AutomationMutableConfigInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  requestId: string
  status: AutomationStatus
}
export interface AutomationUpdateInput extends AutomationMutableConfigInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  expectedRevision: number
}
export interface AutomationSetEnabledInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  expectedRevision: number
  enabled: boolean
}
export interface AutomationRunNowInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  requestId: string
}
export interface AutomationDeleteInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  expectedRevision: number
}
export interface AutomationRunsListInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  cursor?: string
  limit: number
}
export interface AutomationAttentionSummaryInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  cursor?: string
  limit: number
}
export interface AutomationAttentionAcknowledgeInput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  attentionId: string
}

export interface AutomationListOutput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  tasks: AutomationTaskSummary[]
  nextCursor: string | null
  counts: { all: number; active: number; paused: number }
  attentionCount: number
  lastSequence: number
}
export interface AutomationRunsListOutput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  runs: AutomationRun[]
  nextCursor: string | null
}
export interface AutomationAttentionSummaryOutput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  unreadCount: number
  items: AutomationAttention[]
  nextCursor: string | null
  lastSequence: number
}
export interface AutomationAttentionAcknowledgeOutput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  attention: AutomationAttention
}
export interface AutomationDeleteOutput {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  deletedAt: number
}

/** Main-to-Renderer intent emitted after a user clicks a native notification. */
export interface AutomationOpenRequest {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  automationId: string
  runId: string | null
  destination:
    { kind: 'task' } | { kind: 'conversation'; conversationId: string; messageId: string | null }
}

export type AutomationEventKind =
  'created' | 'updated' | 'deleted' | 'run_updated' | 'attention_changed' | 'notification_requested'
export interface AutomationEvent {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  sequence: number
  eventId: string
  kind: AutomationEventKind
  automationId: string
  runId: string | null
  resourceRevision: number | null
  occurredAt: number
}
export interface AutomationResync {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  reason: 'core_started'
  lastSequence: number
  occurredAt: number
}

export type AutomationErrorKind =
  | 'not_found'
  | 'revision_conflict'
  | 'validation'
  | 'run_already_active'
  | 'target_invalid'
  | 'permission_disabled'
  | 'schedule_invalid'
  | 'internal'
export interface AutomationErrorData {
  schemaVersion: typeof AUTOMATION_SCHEMA_VERSION
  type: 'automation'
  code: AutomationErrorKind
  message: string
  automationId: string | null
  currentRevision: number | null
  field: string | null
  retryable: boolean
}

const ID_MAX = 256
const TITLE_MAX = 512
const PROMPT_MAX = 65_536
const SAFE_TEXT_MAX = 4_096
const CURSOR_MAX = 2_048
const RRULE_MAX = 4_096
const ZONE_MAX = 255
const LIST_MAX = 100
const WEEKDAYS = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday'
] as const

function boundedString(
  value: unknown,
  context: string,
  max: number,
  allowEmpty = false,
  allowFormattingControls = false
): string {
  const result = expectString(value, context)
  const byteLength = new TextEncoder().encode(result).byteLength
  const invalidControls = [...result].some((character) => {
    const code = character.charCodeAt(0)
    const isControl = code <= 31 || (code >= 127 && code <= 159)
    return isControl && (!allowFormattingControls || (character !== '\n' && character !== '\t'))
  })
  if ((!allowEmpty && result.trim().length === 0) || byteLength > max || invalidControls) {
    throw invalidProtocolValue(
      context,
      `must contain ${allowEmpty ? 'at most' : '1 to'} ${max} UTF-8 bytes without invalid controls`
    )
  }
  return result
}
function nullableString(
  value: unknown,
  context: string,
  max: number,
  allowFormattingControls = false
): string | null {
  return value === null ? null : boundedString(value, context, max, true, allowFormattingControls)
}
function nullableNonEmptyString(value: unknown, context: string, max: number): string | null {
  return value === null ? null : boundedString(value, context, max)
}
function nullableInteger(value: unknown, context: string): number | null {
  return value === null ? null : expectSafeInteger(value, context, 0)
}
function intInRange(value: unknown, context: string, min: number, max: number): number {
  const result = expectSafeInteger(value, context, min)
  if (result > max) throw invalidProtocolValue(context, `must not exceed ${max}`)
  return result
}
function parseStringArrayEnum<T extends readonly string[]>(
  value: unknown,
  values: T,
  context: string
): T[number][] {
  const array = expectArray(value, context)
  return array.map((item, index) => expectEnum(item, values, `${context}[${index}]`))
}
function parseIntegerArray(value: unknown, context: string, min: number, max: number): number[] {
  return expectArray(value, context).map((item, index) =>
    intInRange(item, `${context}[${index}]`, min, max)
  )
}

export function parseAutomationScheduleInput(value: unknown): AutomationScheduleInput {
  const context = 'Automation schedule'
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['interval', 'daily', 'weekdays', 'weekly', 'custom'] as const,
    `${context}.kind`
  )
  const common = {
    anchorAt: expectSafeInteger(record.anchorAt, `${context}.anchorAt`, 0),
    timezone: boundedString(record.timezone, `${context}.timezone`, ZONE_MAX)
  }
  if (kind === 'interval') {
    expectOnlyKeys(record, ['kind', 'amount', 'unit', 'anchorAt', 'timezone'], context)
    return {
      kind,
      amount: expectSafeInteger(record.amount, `${context}.amount`, 1),
      unit: expectEnum(record.unit, ['minutes', 'hours', 'days'] as const, `${context}.unit`),
      ...common
    }
  }
  if (kind === 'daily' || kind === 'weekdays') {
    expectOnlyKeys(record, ['kind', 'timeMinutes', 'anchorAt', 'timezone'], context)
    return {
      kind,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  if (kind === 'weekly') {
    expectOnlyKeys(record, ['kind', 'weekdays', 'timeMinutes', 'anchorAt', 'timezone'], context)
    return {
      kind,
      weekdays: parseStringArrayEnum(record.weekdays, WEEKDAYS, `${context}.weekdays`),
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  expectOnlyKeys(
    record,
    [
      'kind',
      'frequency',
      'interval',
      'minuteOfHour',
      'timeMinutes',
      'weekdays',
      'monthDays',
      'months',
      'anchorAt',
      'timezone'
    ],
    context
  )
  const frequency = expectEnum(
    record.frequency,
    ['hourly', 'daily', 'weekly', 'monthly', 'yearly'] as const,
    `${context}.frequency`
  )
  const interval = expectSafeInteger(record.interval, `${context}.interval`, 1)
  const present = (field: string): boolean => record[field] !== undefined && record[field] !== null
  const reject = (fields: string[]): void => {
    const unexpected = fields.find(present)
    if (unexpected !== undefined)
      throw invalidProtocolValue(`${context}.${unexpected}`, `is not valid for ${frequency}`)
  }
  if (frequency === 'hourly') {
    reject(['timeMinutes', 'weekdays', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      minuteOfHour: intInRange(record.minuteOfHour, `${context}.minuteOfHour`, 0, 59),
      ...common
    }
  }
  if (frequency === 'daily') {
    reject(['minuteOfHour', 'weekdays', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  if (frequency === 'weekly') {
    reject(['minuteOfHour', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      weekdays: parseStringArrayEnum(record.weekdays, WEEKDAYS, `${context}.weekdays`),
      ...common
    }
  }
  if (frequency === 'monthly') {
    reject(['minuteOfHour', 'weekdays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      monthDays: parseIntegerArray(record.monthDays, `${context}.monthDays`, 1, 31),
      ...common
    }
  }
  reject(['minuteOfHour', 'weekdays'])
  return {
    kind,
    frequency,
    interval,
    timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
    monthDays: parseIntegerArray(record.monthDays, `${context}.monthDays`, 1, 31),
    months: parseIntegerArray(record.months, `${context}.months`, 1, 12),
    ...common
  }
}

function parseDestinationInput(value: unknown): AutomationDestinationInput {
  const context = 'Automation destination input'
  const record = expectRecord(value, context)
  const kind = expectEnum(record.kind, ['new_chat', 'existing_chat'] as const, `${context}.kind`)
  if (kind === 'existing_chat') {
    expectOnlyKeys(record, ['kind', 'conversationId'], context)
    return {
      kind,
      conversationId: boundedString(record.conversationId, `${context}.conversationId`, ID_MAX)
    }
  }
  expectOnlyKeys(record, ['kind', 'projectBinding', 'projectId', 'modelId'], context)
  const projectBinding = expectEnum(
    record.projectBinding,
    ['none', 'project'] as const,
    `${context}.projectBinding`
  )
  const projectId =
    record.projectId == null
      ? null
      : boundedString(record.projectId, `${context}.projectId`, ID_MAX)
  if ((projectBinding === 'none') !== (projectId === null))
    throw invalidProtocolValue(context, 'projectBinding and projectId disagree')
  return {
    kind,
    projectBinding,
    projectId,
    modelId: boundedString(record.modelId, `${context}.modelId`, ID_MAX)
  }
}

function parseReasoning(value: unknown): AutomationReasoningProjection {
  const context = 'Automation reasoning projection'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['source', 'mode', 'effort'], context)
  if (record.source !== 'model_config')
    throw invalidProtocolValue(`${context}.source`, 'must be model_config')
  return {
    source: 'model_config',
    mode: expectEnum(
      record.mode,
      ['provider_default', 'enabled', 'disabled'] as const,
      `${context}.mode`
    ),
    effort: expectEnum(
      record.effort,
      ['provider_default', 'low', 'high', 'max'] as const,
      `${context}.effort`
    )
  }
}

function parseDestination(value: unknown): AutomationDestination {
  const record = expectRecord(value, 'Automation destination')
  if (record.kind === 'existing_chat') {
    const destination = parseDestinationInput(value)
    if (destination.kind !== 'existing_chat') {
      throw invalidProtocolValue('Automation destination', 'invalid existing chat destination')
    }
    return destination
  }
  expectOnlyKeys(
    record,
    ['kind', 'projectBinding', 'projectId', 'modelId', 'reasoning'],
    'Automation destination'
  )
  const base = parseDestinationInput({
    kind: record.kind,
    projectBinding: record.projectBinding,
    projectId: record.projectId,
    modelId: record.modelId
  })
  if (base.kind !== 'new_chat')
    throw invalidProtocolValue('Automation destination', 'invalid new chat destination')
  return { ...base, reasoning: parseReasoning(record.reasoning) }
}

function parsePermissions(value: unknown): AgentPermissions {
  const context = 'Automation resolved permissions'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['read', 'write', 'command', 'commandSafety', 'patch', 'builtinExecution'],
    context
  )
  return {
    read: expectEnum(record.read, ['workspace_only', 'all'] as const, `${context}.read`),
    write: expectEnum(
      record.write,
      ['denied', 'workspace_only', 'all'] as const,
      `${context}.write`
    ),
    command: expectEnum(
      record.command,
      ['require_approval', 'auto_approve'] as const,
      `${context}.command`
    ),
    commandSafety: expectEnum(
      record.commandSafety,
      ['guarded', 'full_access'] as const,
      `${context}.commandSafety`
    ),
    patch: expectEnum(
      record.patch,
      ['require_approval', 'auto_approve'] as const,
      `${context}.patch`
    ),
    builtinExecution: expectEnum(
      record.builtinExecution,
      ['require_approval', 'auto_approve'] as const,
      `${context}.builtinExecution`
    )
  }
}

function parseHealth(value: unknown): AutomationHealth {
  const context = 'Automation health'
  const record = expectRecord(value, context)
  const state = expectEnum(record.state, ['ok', 'blocked'] as const, `${context}.state`)
  if (state === 'ok') {
    expectOnlyKeys(record, ['state'], context)
    return { state }
  }
  expectOnlyKeys(record, ['state', 'code', 'message'], context)
  return {
    state,
    code: expectEnum(
      record.code,
      [
        'target_missing',
        'target_archived',
        'target_invalid',
        'project_missing',
        'project_path_missing',
        'model_missing',
        'model_disabled',
        'model_unavailable',
        'permission_disabled',
        'configuration_invalid',
        'schedule_invalid'
      ] as const,
      `${context}.code`
    ),
    message: boundedString(record.message, `${context}.message`, SAFE_TEXT_MAX, false, true)
  }
}

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

export function parseAutomationTask(value: unknown): AutomationTask {
  const context = 'Automation task'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'automationId',
      'title',
      'prompt',
      'status',
      'health',
      'destination',
      'permissionMode',
      'permissionModeVersion',
      'resolvedPermissions',
      'schedule',
      'scheduleSummary',
      'rrule',
      'timezone',
      'notificationPolicy',
      'targetSnapshot',
      'nextRunAt',
      'lastScheduledAt',
      'lastRunAt',
      'latestRun',
      'attention',
      'revision',
      'createdAt',
      'updatedAt'
    ],
    context
  )
  expectSchemaVersion(record, AUTOMATION_SCHEMA_VERSION, context)
  const snapshot = expectRecord(record.targetSnapshot, `${context}.targetSnapshot`)
  expectOnlyKeys(
    snapshot,
    ['projectName', 'conversationTitle', 'modelDisplayName'],
    `${context}.targetSnapshot`
  )
  if (record.permissionModeVersion !== AUTOMATION_PERMISSION_MODE_VERSION)
    throw invalidProtocolValue(
      `${context}.permissionModeVersion`,
      'unsupported permission mode version'
    )
  const automationId = boundedString(record.automationId, `${context}.automationId`, ID_MAX)
  const status = expectEnum(record.status, ['active', 'paused'] as const, `${context}.status`)
  const health = parseHealth(record.health)
  const schedule = parseAutomationScheduleInput(record.schedule)
  const timezone = boundedString(record.timezone, `${context}.timezone`, ZONE_MAX)
  const nextRunAt = nullableInteger(record.nextRunAt, `${context}.nextRunAt`)
  const latestRun = record.latestRun === null ? null : parseAutomationRun(record.latestRun)
  const attention = record.attention === null ? null : parseAutomationAttention(record.attention)
  if (schedule.timezone !== timezone)
    throw invalidProtocolValue(`${context}.timezone`, 'must match schedule.timezone')
  if (latestRun !== null && latestRun.automationId !== automationId)
    throw invalidProtocolValue(`${context}.latestRun.automationId`, 'must match automationId')
  if (attention !== null && attention.automationId !== automationId)
    throw invalidProtocolValue(`${context}.attention.automationId`, 'must match automationId')
  if ((status === 'paused' || health.state === 'blocked') && nextRunAt !== null)
    throw invalidProtocolValue(
      `${context}.nextRunAt`,
      'must be null while the task is paused or blocked'
    )
  return {
    schemaVersion: AUTOMATION_SCHEMA_VERSION,
    automationId,
    title: boundedString(record.title, `${context}.title`, TITLE_MAX),
    prompt: boundedString(record.prompt, `${context}.prompt`, PROMPT_MAX, false, true),
    status,
    health,
    destination: parseDestination(record.destination),
    permissionMode: expectEnum(
      record.permissionMode,
      ['default', 'full', 'custom'] as const,
      `${context}.permissionMode`
    ),
    permissionModeVersion: AUTOMATION_PERMISSION_MODE_VERSION,
    resolvedPermissions: parsePermissions(record.resolvedPermissions),
    schedule,
    scheduleSummary: boundedString(
      record.scheduleSummary,
      `${context}.scheduleSummary`,
      SAFE_TEXT_MAX
    ),
    rrule: boundedString(record.rrule, `${context}.rrule`, RRULE_MAX),
    timezone,
    notificationPolicy: expectEnum(
      record.notificationPolicy,
      ['all_runs', 'unsuccessful_only', 'important_updates'] as const,
      `${context}.notificationPolicy`
    ),
    targetSnapshot: {
      projectName: nullableString(
        snapshot.projectName,
        `${context}.targetSnapshot.projectName`,
        TITLE_MAX
      ),
      conversationTitle: nullableString(
        snapshot.conversationTitle,
        `${context}.targetSnapshot.conversationTitle`,
        TITLE_MAX
      ),
      modelDisplayName: nullableString(
        snapshot.modelDisplayName,
        `${context}.targetSnapshot.modelDisplayName`,
        TITLE_MAX
      )
    },
    nextRunAt,
    lastScheduledAt: nullableInteger(record.lastScheduledAt, `${context}.lastScheduledAt`),
    lastRunAt: nullableInteger(record.lastRunAt, `${context}.lastRunAt`),
    latestRun,
    attention,
    revision: expectSafeInteger(record.revision, `${context}.revision`, 0),
    createdAt: expectSafeInteger(record.createdAt, `${context}.createdAt`, 0),
    updatedAt: expectSafeInteger(record.updatedAt, `${context}.updatedAt`, 0)
  }
}

function parseConfig(
  record: Record<string, unknown>,
  context: string
): AutomationMutableConfigInput {
  if (record.permissionModeVersion !== AUTOMATION_PERMISSION_MODE_VERSION)
    throw invalidProtocolValue(
      `${context}.permissionModeVersion`,
      'unsupported permission mode version'
    )
  return {
    title: boundedString(record.title, `${context}.title`, TITLE_MAX),
    prompt: boundedString(record.prompt, `${context}.prompt`, PROMPT_MAX, false, true),
    destination: parseDestinationInput(record.destination),
    permissionMode: expectEnum(
      record.permissionMode,
      ['default', 'full', 'custom'] as const,
      `${context}.permissionMode`
    ),
    permissionModeVersion: AUTOMATION_PERMISSION_MODE_VERSION,
    schedule: parseAutomationScheduleInput(record.schedule),
    notificationPolicy: expectEnum(
      record.notificationPolicy,
      ['all_runs', 'unsuccessful_only', 'important_updates'] as const,
      `${context}.notificationPolicy`
    )
  }
}

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
