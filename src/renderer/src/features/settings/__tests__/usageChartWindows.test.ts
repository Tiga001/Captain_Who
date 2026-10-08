import { describe, expect, it } from 'vitest'
import { getUsageChartWindows } from '../pages/usageChartWindows'

describe('usage chart calendar windows', () => {
  it('uses inclusive contiguous local days across spring and autumn DST transitions', () => {
    const original = process.env.TZ
    process.env.TZ = 'America/New_York'
    try {
      for (const [month, day, hours] of [
        [2, 8, 23],
        [10, 1, 25]
      ]) {
        const now = new Date(2026, month, day, 12).getTime()
        const windows = getUsageChartWindows('last7Days', now)
        expect(windows).toHaveLength(7)
        expect(windows[6].to - windows[6].from + 1).toBe(hours * 60 * 60 * 1000)
        for (let index = 0; index < windows.length; index++) {
          expect(new Date(windows[index].from).getHours()).toBe(0)
          expect(new Date(windows[index].to).getHours()).toBe(23)
          if (index) expect(windows[index - 1].to + 1).toBe(windows[index].from)
        }
      }
    } finally {
      if (original === undefined) delete process.env.TZ
      else process.env.TZ = original
    }
  })
  it('keeps 30 daily or 12 natural monthly buckets, including leap years', () => {
    const now = new Date(2024, 1, 29, 12).getTime()
    expect(getUsageChartWindows('last30Days', now)).toHaveLength(30)
    const year = getUsageChartWindows('lastYear', now)
    expect(year).toHaveLength(12)
    expect(new Date(year[11].from).getDate()).toBe(1)
    expect(new Date(year[11].to).getDate()).toBe(29)
    for (let index = 1; index < year.length; index++)
      expect(year[index - 1].to + 1).toBe(year[index].from)
  })
})
