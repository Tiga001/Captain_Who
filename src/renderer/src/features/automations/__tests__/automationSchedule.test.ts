import { describe, expect, it } from 'vitest'
import type { AutomationScheduleInput } from '@mycopilot/protocol'
import {
  createDefaultAutomationSchedule,
  formatTimeMinutes,
  validateSchedule,
  withSystemTimeZone
} from '../automationSchedule'

const common = { anchorAt: 1_777_777_777_000, timezone: 'Asia/Shanghai' }

describe('automation schedule validation and presentation', () => {
  it.each([0, 30, 59])('supports hourly schedules at minute %s', (minuteOfHour) => {
    const schedule = {
      kind: 'custom',
      frequency: 'hourly',
      interval: 1,
      minuteOfHour,
      ...common
    } satisfies AutomationScheduleInput
    expect(validateSchedule(schedule)).toEqual({ valid: true, errors: {} })
  })

  it('uses the system IANA timezone for a newly created schedule', () => {
    expect(createDefaultAutomationSchedule().timezone).toBe(
      Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
    )
  })

  it('replaces a saved timezone with the current computer timezone before submission', () => {
    expect(
      withSystemTimeZone({
        kind: 'daily',
        timeMinutes: 540,
        anchorAt: common.anchorAt,
        timezone: 'Etc/GMT+12'
      }).timezone
    ).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC')
  })

  it('rejects invalid schedule fields at their specific form paths', () => {
    expect(
      validateSchedule({
        kind: 'custom',
        frequency: 'hourly',
        interval: 0,
        minuteOfHour: 60,
        anchorAt: -1,
        timezone: 'Not/A_Zone'
      })
    ).toEqual({
      valid: false,
      errors: {
        schedule: 'anchor_invalid',
        timezone: 'timezone_invalid',
        scheduleInterval: 'interval_invalid',
        scheduleMinute: 'minute_invalid'
      }
    })
  })

  it('formats backend non-quarter values without rounding', () => {
    expect(formatTimeMinutes(547)).toBe('09:07')
  })
})
