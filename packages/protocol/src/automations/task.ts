import type {
  AutomationDestinationInput,
  AutomationReasoningProjection,
  AutomationDestination,
  AutomationHealth,
  AutomationTask,
  AutomationMutableConfigInput
} from './types'
import {
  boundedString,
  ID_MAX,
  SAFE_TEXT_MAX,
  ZONE_MAX,
  nullableInteger,
  TITLE_MAX,
  PROMPT_MAX,
  RRULE_MAX,
  nullableString
} from './validation'
import {
  expectRecord,
  expectEnum,
  expectOnlyKeys,
  invalidProtocolValue,
  expectSchemaVersion,
  expectSafeInteger
} from '../skills/validation'
import type { AgentPermissions } from '../agent'
import { AUTOMATION_SCHEMA_VERSION, AUTOMATION_PERMISSION_MODE_VERSION } from './constants'
import { parseAutomationScheduleInput } from './schedule'
import { parseAutomationRun, parseAutomationAttention } from './run'

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

export function parseConfig(
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
