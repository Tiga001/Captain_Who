import type { AgentUsageWindow } from '@mycopilot/protocol'

export type UsageChartRange = 'last7Days' | 'last30Days' | 'lastYear'

export function getUsageChartWindows(range: UsageChartRange, now = Date.now()): AgentUsageWindow[] {
  const date = new Date(now)
  const year = date.getFullYear()
  const month = date.getMonth()
  if (range === 'lastYear')
    return Array.from({ length: 12 }, (_, index) => ({
      from: new Date(year, month - 11 + index, 1).getTime(),
      to: new Date(year, month - 10 + index, 1).getTime() - 1
    }))
  const days = range === 'last7Days' ? 7 : 30
  // Calendar arithmetic keeps adjacent local midnights contiguous across 23/25-hour DST days.
  const firstDay = date.getDate() - days + 1
  return Array.from({ length: days }, (_, index) => ({
    from: new Date(year, month, firstDay + index).getTime(),
    to: new Date(year, month, firstDay + index + 1).getTime() - 1
  }))
}
