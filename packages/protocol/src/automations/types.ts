import type { ProviderReasoningMode, ProviderReasoningEffort } from '../storage'
import type { AUTOMATION_SCHEMA_VERSION, AUTOMATION_PERMISSION_MODE_VERSION } from './constants'
import type { AgentPermissions } from '../agent'

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
