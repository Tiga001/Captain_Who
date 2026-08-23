import type { AutomationScheduleInput, AutomationWeekday } from '@mycopilot/protocol'
import type { AutomationFieldErrors, StructuredAutomationSchedule } from './automationTypes'

export const AUTOMATION_WEEKDAYS: readonly AutomationWeekday[] = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday'
]

export interface AutomationTimeOption {
  label: string
  value: number
}

export interface ScheduleValidationResult {
  valid: boolean
  errors: AutomationFieldErrors
}

export interface ScheduleSummaryLabels {
  interval(amount: number, unit: 'minutes' | 'hours' | 'days'): string
  daily(time: string): string
  weekdays(time: string): string
  weekly(weekdays: readonly AutomationWeekday[], time: string): string
  customHourly(interval: number, minute: number): string
  customDaily(interval: number, time: string): string
  customWeekly(interval: number, weekdays: readonly AutomationWeekday[], time: string): string
  customMonthly(interval: number, monthDays: readonly number[], time: string): string
  customYearly(
    interval: number,
    months: readonly number[],
    monthDays: readonly number[],
    time: string
  ): string
}

const DEFAULT_SUMMARY_LABELS: ScheduleSummaryLabels = {
  interval: (amount, unit) => `Every ${amount} ${unit}`,
  daily: (time) => `Daily at ${time}`,
  weekdays: (time) => `Weekdays at ${time}`,
  weekly: (weekdays, time) => `${weekdays.join(', ')} at ${time}`,
  customHourly: (interval, minute) => `Every ${interval} hour(s), at minute ${minute}`,
  customDaily: (interval, time) => `Every ${interval} day(s) at ${time}`,
  customWeekly: (interval, weekdays, time) =>
    `Every ${interval} week(s), ${weekdays.join(', ')} at ${time}`,
  customMonthly: (interval, monthDays, time) =>
    `Every ${interval} month(s), day ${monthDays.join(', ')} at ${time}`,
  customYearly: (interval, months, monthDays, time) =>
    `Every ${interval} year(s), month ${months.join(', ')}, day ${monthDays.join(', ')} at ${time}`
}

function uniqueSortedNumbers(values: readonly number[]): number[] {
  return [...new Set(values)].sort((left, right) => left - right)
}

function uniqueSortedWeekdays(values: readonly AutomationWeekday[]): AutomationWeekday[] {
  const selected = new Set(values)
  return AUTOMATION_WEEKDAYS.filter((weekday) => selected.has(weekday))
}

function cloneAndNormalizeSchedule(schedule: AutomationScheduleInput): AutomationScheduleInput {
  const common = { anchorAt: schedule.anchorAt, timezone: schedule.timezone }
  if (schedule.kind === 'interval') {
    return { kind: 'interval', amount: schedule.amount, unit: schedule.unit, ...common }
  }
  if (schedule.kind === 'daily') {
    return { kind: 'daily', timeMinutes: schedule.timeMinutes, ...common }
  }
  if (schedule.kind === 'weekdays') {
    return { kind: 'weekdays', timeMinutes: schedule.timeMinutes, ...common }
  }
  if (schedule.kind === 'weekly') {
    return {
      kind: 'weekly',
      weekdays: uniqueSortedWeekdays(schedule.weekdays),
      timeMinutes: schedule.timeMinutes,
      ...common
    }
  }
  const custom = schedule as Extract<AutomationScheduleInput, { kind: 'custom' }>
  if (custom.frequency === 'hourly') {
    return {
      kind: 'custom',
      frequency: 'hourly',
      interval: custom.interval,
      minuteOfHour: custom.minuteOfHour,
      ...common
    }
  }
  if (custom.frequency === 'daily') {
    return {
      kind: 'custom',
      frequency: 'daily',
      interval: custom.interval,
      timeMinutes: custom.timeMinutes,
      ...common
    }
  }
  if (custom.frequency === 'weekly') {
    return {
      kind: 'custom',
      frequency: 'weekly',
      interval: custom.interval,
      weekdays: uniqueSortedWeekdays(custom.weekdays),
      timeMinutes: custom.timeMinutes,
      ...common
    }
  }
  if (custom.frequency === 'monthly') {
    return {
      kind: 'custom',
      frequency: 'monthly',
      interval: custom.interval,
      monthDays: uniqueSortedNumbers(custom.monthDays),
      timeMinutes: custom.timeMinutes,
      ...common
    }
  }
  return {
    kind: 'custom',
    frequency: 'yearly',
    interval: custom.interval,
    months: uniqueSortedNumbers(custom.months),
    monthDays: uniqueSortedNumbers(custom.monthDays),
    timeMinutes: custom.timeMinutes,
    ...common
  }
}

/** Removes hidden/stale form fields and returns the exact public protocol shape. */
export function structuredScheduleToProtocol(
  schedule: StructuredAutomationSchedule
): AutomationScheduleInput {
  return cloneAndNormalizeSchedule(schedule)
}

/** Creates a form-owned clone so editing never mutates an authoritative task object. */
export function protocolScheduleToStructured(
  schedule: AutomationScheduleInput
): StructuredAutomationSchedule {
  return cloneAndNormalizeSchedule(schedule)
}

export function getSystemTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
}

export function nextQuarterHourAnchor(now = Date.now()): number {
  const quarterHour = 15 * 60 * 1_000
  return Math.ceil((now + 1) / quarterHour) * quarterHour
}

export function localTimeMinutes(timestamp: number): number {
  const date = new Date(timestamp)
  return date.getHours() * 60 + date.getMinutes()
}

export function createDefaultAutomationSchedule(
  now = Date.now(),
  timezone = getSystemTimeZone()
): AutomationScheduleInput {
  const anchorAt = nextQuarterHourAnchor(now)
  return {
    kind: 'daily',
    timeMinutes: localTimeMinutes(anchorAt),
    anchorAt,
    timezone
  }
}

export function formatTimeMinutes(timeMinutes: number): string {
  const normalized = Math.max(0, Math.min(1439, Math.trunc(timeMinutes)))
  return `${String(Math.floor(normalized / 60)).padStart(2, '0')}:${String(normalized % 60).padStart(2, '0')}`
}

export function generateQuarterHourOptions(): AutomationTimeOption[] {
  return Array.from({ length: 96 }, (_, index) => ({
    value: index * 15,
    label: formatTimeMinutes(index * 15)
  }))
}

export function isValidIanaTimeZone(timezone: string): boolean {
  if (!timezone.trim()) return false
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: timezone }).format(0)
    return true
  } catch {
    return false
  }
}

function isPositiveInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value > 0
}

function isTimeMinutes(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0 && value <= 1439
}

function validAnchor(anchorAt: number): boolean {
  return Number.isSafeInteger(anchorAt) && anchorAt >= 0
}

export function validateSchedule(schedule: StructuredAutomationSchedule): ScheduleValidationResult {
  const errors: AutomationFieldErrors = {}
  if (!validAnchor(schedule.anchorAt)) errors.schedule = 'anchor_invalid'
  if (!isValidIanaTimeZone(schedule.timezone)) errors.timezone = 'timezone_invalid'

  if (schedule.kind === 'interval') {
    if (!isPositiveInteger(schedule.amount)) errors.scheduleAmount = 'amount_invalid'
  } else if (schedule.kind === 'daily') {
    if (!isTimeMinutes(schedule.timeMinutes)) errors.scheduleTime = 'time_invalid'
  } else if (schedule.kind === 'weekdays') {
    if (!isTimeMinutes(schedule.timeMinutes)) errors.scheduleTime = 'time_invalid'
  } else if (schedule.kind === 'weekly') {
    if (schedule.weekdays.length === 0) errors.scheduleWeekdays = 'weekdays_required'
    if (uniqueSortedWeekdays(schedule.weekdays).length !== schedule.weekdays.length) {
      errors.scheduleWeekdays = 'weekdays_invalid'
    }
    if (!isTimeMinutes(schedule.timeMinutes)) errors.scheduleTime = 'time_invalid'
  } else {
    const custom = schedule as Extract<AutomationScheduleInput, { kind: 'custom' }>
    if (!isPositiveInteger(custom.interval)) errors.scheduleInterval = 'interval_invalid'
    if (custom.frequency === 'hourly') {
      if (
        !Number.isSafeInteger(custom.minuteOfHour) ||
        custom.minuteOfHour < 0 ||
        custom.minuteOfHour > 59
      ) {
        errors.scheduleMinute = 'minute_invalid'
      }
    } else {
      if (!isTimeMinutes(custom.timeMinutes)) errors.scheduleTime = 'time_invalid'
      if (custom.frequency === 'weekly' && custom.weekdays.length === 0) {
        errors.scheduleWeekdays = 'weekdays_required'
      }
      if (custom.frequency === 'monthly') {
        if (
          custom.monthDays.length === 0 ||
          custom.monthDays.some((day) => !Number.isSafeInteger(day) || day < 1 || day > 31)
        ) {
          errors.scheduleMonthDays = 'month_days_invalid'
        }
      }
      if (custom.frequency === 'yearly') {
        if (
          custom.months.length === 0 ||
          custom.months.some((month) => !Number.isSafeInteger(month) || month < 1 || month > 12)
        ) {
          errors.scheduleMonths = 'months_invalid'
        }
        if (
          custom.monthDays.length === 0 ||
          custom.monthDays.some((day) => !Number.isSafeInteger(day) || day < 1 || day > 31)
        ) {
          errors.scheduleMonthDays = 'month_days_invalid'
        }
      }
    }
  }

  return { valid: Object.keys(errors).length === 0, errors }
}

export function formatScheduleSummary(
  schedule: AutomationScheduleInput,
  labels: ScheduleSummaryLabels = DEFAULT_SUMMARY_LABELS
): string {
  if (schedule.kind === 'interval') return labels.interval(schedule.amount, schedule.unit)
  if (schedule.kind === 'daily') return labels.daily(formatTimeMinutes(schedule.timeMinutes))
  if (schedule.kind === 'weekdays') return labels.weekdays(formatTimeMinutes(schedule.timeMinutes))
  if (schedule.kind === 'weekly') {
    return labels.weekly(schedule.weekdays, formatTimeMinutes(schedule.timeMinutes))
  }
  const custom = schedule as Extract<AutomationScheduleInput, { kind: 'custom' }>
  if (custom.frequency === 'hourly') {
    return labels.customHourly(custom.interval, custom.minuteOfHour)
  }
  if (custom.frequency === 'daily') {
    return labels.customDaily(custom.interval, formatTimeMinutes(custom.timeMinutes))
  }
  if (custom.frequency === 'weekly') {
    return labels.customWeekly(
      custom.interval,
      custom.weekdays,
      formatTimeMinutes(custom.timeMinutes)
    )
  }
  if (custom.frequency === 'monthly') {
    return labels.customMonthly(
      custom.interval,
      custom.monthDays,
      formatTimeMinutes(custom.timeMinutes)
    )
  }
  return labels.customYearly(
    custom.interval,
    custom.months,
    custom.monthDays,
    formatTimeMinutes(custom.timeMinutes)
  )
}
