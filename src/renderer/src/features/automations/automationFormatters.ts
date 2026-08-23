import type { AutomationRunStatus, AutomationTask } from './automationTypes'

export function formatAutomationDateTime(
  timestamp: number | null,
  locale: string,
  timezone?: string
): string | null {
  if (timestamp === null) return null
  return new Intl.DateTimeFormat(locale, {
    dateStyle: 'medium',
    timeStyle: 'short',
    ...(timezone ? { timeZone: timezone } : {})
  }).format(timestamp)
}

export function formatAutomationRelativeTime(
  timestamp: number | null,
  now: number,
  locale: string
): string | null {
  if (timestamp === null) return null
  const deltaSeconds = Math.round((timestamp - now) / 1_000)
  const absoluteSeconds = Math.abs(deltaSeconds)
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ['year', 31_536_000],
    ['month', 2_592_000],
    ['week', 604_800],
    ['day', 86_400],
    ['hour', 3_600],
    ['minute', 60]
  ]
  const [unit, seconds] = units.find(([, size]) => absoluteSeconds >= size) ?? ['second', 1]
  return new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(
    Math.round(deltaSeconds / seconds),
    unit
  )
}

export function isAutomationRunActive(status: AutomationRunStatus | undefined): boolean {
  return (
    status === 'queued' ||
    status === 'starting' ||
    status === 'running' ||
    status === 'waiting_for_approval'
  )
}

export function taskNeedsAttention(task: AutomationTask): boolean {
  return task.attention !== null && task.attention.readAt === null
}
