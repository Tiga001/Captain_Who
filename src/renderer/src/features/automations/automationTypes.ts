import { AUTOMATION_PERMISSION_MODE_VERSION } from '@mycopilot/protocol'
import type {
  AutomationAttention,
  AutomationAttentionKind,
  AutomationCustomFrequency,
  AutomationDestination,
  AutomationDestinationInput,
  AutomationErrorKind,
  AutomationEvent,
  AutomationHealth,
  AutomationIntervalUnit,
  AutomationMutableConfigInput,
  AutomationNotificationPolicy,
  AutomationPermissionMode,
  AutomationReasoningProjection,
  AutomationResync,
  AutomationRun,
  AutomationRunStatus,
  AutomationRunTriggerKind,
  AutomationSchedule,
  AutomationScheduleInput,
  AutomationStatus,
  AutomationTask,
  AutomationTaskSummary,
  AutomationWeekday
} from '@mycopilot/protocol'

export type {
  AutomationAttention,
  AutomationAttentionKind,
  AutomationCustomFrequency,
  AutomationDestination,
  AutomationDestinationInput,
  AutomationErrorKind,
  AutomationEvent,
  AutomationHealth,
  AutomationIntervalUnit,
  AutomationMutableConfigInput,
  AutomationNotificationPolicy,
  AutomationPermissionMode,
  AutomationReasoningProjection,
  AutomationResync,
  AutomationRun,
  AutomationRunStatus,
  AutomationRunTriggerKind,
  AutomationSchedule,
  AutomationScheduleInput,
  AutomationStatus,
  AutomationTask,
  AutomationTaskSummary,
  AutomationWeekday
}

export type AutomationFilter = 'all' | AutomationStatus
export type AutomationLoadStatus = 'idle' | 'loading' | 'ready' | 'error'
export type AutomationMutationKind = 'update' | 'set_enabled' | 'run_now' | 'delete'

/**
 * Renderer-owned create draft. Resolved permissions and reasoning are intentionally absent:
 * Core derives both from trusted settings/model state.
 */
export interface AutomationDraft extends AutomationMutableConfigInput {
  status: AutomationStatus
}

export type AutomationUpdateDraft = AutomationMutableConfigInput
export type AutomationMutationInput = AutomationMutableConfigInput

export interface AutomationFieldErrors {
  title?: string
  prompt?: string
  destination?: string
  projectId?: string
  modelId?: string
  conversationId?: string
  permissionMode?: string
  schedule?: string
  scheduleAmount?: string
  scheduleInterval?: string
  scheduleMinute?: string
  scheduleTime?: string
  scheduleWeekdays?: string
  scheduleMonthDays?: string
  scheduleMonths?: string
  timezone?: string
  notificationPolicy?: string
}

export interface AutomationValidationResult {
  valid: boolean
  errors: AutomationFieldErrors
}

export type StructuredAutomationSchedule = AutomationScheduleInput

export interface AutomationMutationTarget {
  automationId: string
  revision: number
}

export interface AutomationListQuery {
  status?: AutomationStatus
  query?: string
  cursor?: string
  limit?: number
}

export interface AutomationRunsQuery {
  cursor?: string
  limit?: number
}

export interface AutomationAttentionQuery {
  cursor?: string
  limit?: number
}

export interface AutomationErrorDetails {
  code: AutomationErrorKind | 'transport'
  message: string
  automationId: string | null
  currentRevision: number | null
  field: string | null
  retryable: boolean
}

export const AUTOMATION_PERMISSION_VERSION = AUTOMATION_PERMISSION_MODE_VERSION
