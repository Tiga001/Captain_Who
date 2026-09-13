import type {
  AutomationHealth,
  AutomationRun,
  AutomationRunStatus,
  AutomationRunTriggerKind,
  AutomationTask,
  AutomationWeekday
} from '@mycopilot/protocol'
import type { AppLanguage, TranslationKey } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'
import { formatTranslation } from '../../config/translationFormat'
import { formatScheduleSummary } from './automationSchedule'

const RUN_STATUS_KEYS: Record<AutomationRunStatus, TranslationKey> = {
  queued: 'automation.statusQueued',
  starting: 'automation.statusStarting',
  running: 'automation.statusRunning',
  waiting_for_approval: 'automation.statusWaitingApproval',
  completed: 'automation.statusCompleted',
  failed: 'automation.statusFailed',
  cancelled: 'automation.statusCancelled'
}

const TRIGGER_KEYS: Record<AutomationRunTriggerKind, TranslationKey> = {
  scheduled: 'automation.triggerScheduled',
  manual: 'automation.triggerManual',
  recovery: 'automation.triggerRecovery'
}

const HEALTH_KEYS: Record<Extract<AutomationHealth, { state: 'blocked' }>['code'], TranslationKey> =
  {
    target_missing: 'automation.healthTargetMissing',
    target_archived: 'automation.healthTargetArchived',
    target_invalid: 'automation.healthTargetInvalid',
    project_missing: 'automation.healthProjectMissing',
    project_path_missing: 'automation.healthProjectPathMissing',
    model_missing: 'automation.healthModelMissing',
    model_disabled: 'automation.healthModelDisabled',
    permission_disabled: 'automation.healthPermissionDisabled',
    configuration_invalid: 'automation.healthConfigurationInvalid',
    schedule_invalid: 'automation.healthScheduleInvalid'
  }

export const WEEKDAY_KEYS: Record<AutomationWeekday, TranslationKey> = {
  monday: 'automation.weekdayMonday',
  tuesday: 'automation.weekdayTuesday',
  wednesday: 'automation.weekdayWednesday',
  thursday: 'automation.weekdayThursday',
  friday: 'automation.weekdayFriday',
  saturday: 'automation.weekdaySaturday',
  sunday: 'automation.weekdaySunday'
}

export const WEEKDAYS: AutomationWeekday[] = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday'
]

export function runStatusLabel(t: Translate, status: AutomationRunStatus): string {
  return t(RUN_STATUS_KEYS[status])
}

export function triggerLabel(t: Translate, trigger: AutomationRunTriggerKind): string {
  return t(TRIGGER_KEYS[trigger])
}

export function healthMessage(t: Translate, health: AutomationHealth): string {
  return health.state === 'blocked' ? t(HEALTH_KEYS[health.code]) : ''
}

export function localizedScheduleSummary(
  task: AutomationTask,
  t: Translate,
  language: AppLanguage
): string {
  const usesIdeographicListSeparator =
    language === 'zh-CN' || language === 'zh-TW' || language === 'ja-JP'
  const weekdayList = (weekdays: readonly AutomationWeekday[]) =>
    weekdays
      .map((weekday) => t(WEEKDAY_KEYS[weekday]))
      .join(usesIdeographicListSeparator ? '、' : ', ')
  return formatScheduleSummary(task.schedule, {
    interval: (amount, unit) =>
      formatTranslation(t, 'automation.summaryInterval', {
        amount,
        unit: t(
          unit === 'minutes'
            ? 'automation.intervalMinutes'
            : unit === 'hours'
              ? 'automation.intervalHours'
              : 'automation.intervalDays'
        )
      }),
    daily: (time) => formatTranslation(t, 'automation.summaryDaily', { time }),
    weekdays: (time) => formatTranslation(t, 'automation.summaryWeekdays', { time }),
    weekly: (weekdays, time) =>
      formatTranslation(t, 'automation.summaryWeekly', {
        weekdays: weekdayList(weekdays),
        time
      }),
    customHourly: (interval, minute) =>
      formatTranslation(t, 'automation.summaryCustomHourly', { interval, minute }),
    customDaily: (interval, time) =>
      formatTranslation(t, 'automation.summaryCustomDaily', { interval, time }),
    customWeekly: (interval, weekdays, time) =>
      formatTranslation(t, 'automation.summaryCustomWeekly', {
        interval,
        weekdays: weekdayList(weekdays),
        time
      }),
    customMonthly: (interval, monthDays, time) =>
      formatTranslation(t, 'automation.summaryCustomMonthly', {
        interval,
        monthDays: monthDays.join(', '),
        time
      }),
    customYearly: (interval, months, monthDays, time) =>
      formatTranslation(t, 'automation.summaryCustomYearly', {
        interval,
        months: months.join(', '),
        monthDays: monthDays.join(', '),
        time
      })
  })
}

export function runErrorMessage(t: Translate, run: AutomationRun): string | null {
  if (!run.errorCode && !run.errorMessage) return null
  if (run.errorCode === 'ACCOUNT_LOGIN_REQUIRED') return t('license.runLoginRequired')
  if (run.errorCode === 'ACCOUNT_LICENSE_REQUIRED') return t('license.runLicenseRequired')
  if (run.errorCode === 'ACCOUNT_LICENSE_UNAVAILABLE') return t('license.runVerificationRequired')
  if (run.errorCode === 'permission_disabled') return t('automation.runErrorPermission')
  if (
    run.errorCode === 'target_invalid' ||
    run.errorCode === 'target_missing' ||
    run.errorCode === 'target_archived' ||
    run.errorCode === 'project_missing' ||
    run.errorCode === 'model_missing'
  ) {
    return t('automation.runErrorTarget')
  }
  if (run.errorCode === 'agent_turn_failed') return t('automation.runErrorAgent')
  return run.errorMessage || t('automation.runErrorGeneric')
}

export function formatAbsoluteDateTime(
  timestamp: number,
  language: AppLanguage,
  timezone?: string
): string {
  try {
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short',
      timeZone: timezone
    }).format(timestamp)
  } catch {
    return new Intl.DateTimeFormat(language, {
      dateStyle: 'medium',
      timeStyle: 'short'
    }).format(timestamp)
  }
}

export function formatRelativeRunTime(
  timestamp: number,
  language: AppLanguage,
  now = Date.now()
): string {
  const deltaSeconds = Math.round((timestamp - now) / 1000)
  const absoluteSeconds = Math.abs(deltaSeconds)
  const formatter = new Intl.RelativeTimeFormat(language, { numeric: 'auto' })
  if (absoluteSeconds < 90) return formatter.format(deltaSeconds, 'second')
  const minutes = Math.round(deltaSeconds / 60)
  if (Math.abs(minutes) < 90) return formatter.format(minutes, 'minute')
  const hours = Math.round(deltaSeconds / 3_600)
  if (Math.abs(hours) < 36) return formatter.format(hours, 'hour')
  return formatter.format(Math.round(deltaSeconds / 86_400), 'day')
}

export function taskSecondaryText(
  task: AutomationTask,
  t: Translate,
  language: AppLanguage
): string {
  if (task.health.state === 'blocked') return healthMessage(t, task.health)
  const runStatus = task.latestRun?.status
  if (runStatus === 'waiting_for_approval') return t('automation.statusWaitingApproval')
  if (runStatus === 'queued' || runStatus === 'starting' || runStatus === 'running') {
    return runStatusLabel(t, runStatus)
  }

  const parts = [localizedScheduleSummary(task, t, language)]
  if (task.latestRun?.status === 'failed') parts.push(t('automation.lastRunFailed'))
  if (task.status === 'active' && task.nextRunAt !== null) {
    parts.push(
      formatTranslation(t, 'automation.nextRun', {
        time: formatRelativeRunTime(task.nextRunAt, language)
      })
    )
  }
  return parts.filter(Boolean).join(' · ')
}

export function isTaskRunActive(task: AutomationTask): boolean {
  const status = task.latestRun?.status
  return (
    status === 'queued' ||
    status === 'starting' ||
    status === 'running' ||
    status === 'waiting_for_approval'
  )
}
