import { describe, expect, it } from 'vitest'
import type { AutomationScheduleInput } from '@mycopilot/protocol'
import {
  formatTimeMinutes,
  generateQuarterHourOptions,
  protocolScheduleToStructured,
  structuredScheduleToProtocol,
  validateSchedule
} from '../automationSchedule'

const common = { anchorAt: 1_777_777_777_000, timezone: 'Asia/Shanghai' }

describe('automation schedule form protocol adapter', () => {
  it.each([0, 30, 59])('supports hourly schedules at minute %s', (minuteOfHour) => {
    const schedule = {
      kind: 'custom',
      frequency: 'hourly',
      interval: 1,
      minuteOfHour,
      ...common
    } satisfies AutomationScheduleInput
    expect(validateSchedule(schedule)).toEqual({ valid: true, errors: {} })
    expect(structuredScheduleToProtocol(schedule)).toEqual(schedule)
  })

  it('round-trips every three days without adding unrelated fields', () => {
    const schedule = {
      kind: 'custom',
      frequency: 'daily',
      interval: 3,
      timeMinutes: 150,
      ...common
    } satisfies AutomationScheduleInput
    const roundTrip = structuredScheduleToProtocol(protocolScheduleToStructured(schedule))
    expect(roundTrip).toEqual(schedule)
    expect(roundTrip).not.toHaveProperty('minuteOfHour')
    expect(roundTrip).not.toHaveProperty('weekdays')
  })

  it('normalizes selected weekdays into canonical order', () => {
    const schedule = {
      kind: 'custom',
      frequency: 'weekly',
      interval: 2,
      weekdays: ['friday', 'monday', 'wednesday'],
      timeMinutes: 540,
      ...common
    } satisfies AutomationScheduleInput
    expect(structuredScheduleToProtocol(schedule)).toMatchObject({
      weekdays: ['monday', 'wednesday', 'friday']
    })
  })

  it('preserves monthly dates including the 31st for backend skip semantics', () => {
    const schedule = {
      kind: 'custom',
      frequency: 'monthly',
      interval: 1,
      monthDays: [31, 1, 12],
      timeMinutes: 540,
      ...common
    } satisfies AutomationScheduleInput
    expect(structuredScheduleToProtocol(schedule)).toMatchObject({ monthDays: [1, 12, 31] })
  })

  it('preserves February 29 for backend leap-year skip semantics', () => {
    const schedule = {
      kind: 'custom',
      frequency: 'yearly',
      interval: 1,
      months: [2],
      monthDays: [29],
      timeMinutes: 540,
      ...common
    } satisfies AutomationScheduleInput
    expect(structuredScheduleToProtocol(schedule)).toEqual(schedule)
  })

  it('keeps the saved IANA timezone across protocol conversion', () => {
    const schedule = {
      kind: 'weekdays',
      timeMinutes: 540,
      ...common
    } satisfies AutomationScheduleInput
    expect(protocolScheduleToStructured(schedule).timezone).toBe('Asia/Shanghai')
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

  it('provides exactly 96 quarter-hour options while formatting backend non-quarter values', () => {
    const options = generateQuarterHourOptions()
    expect(options).toHaveLength(96)
    expect(options[0]).toEqual({ value: 0, label: '00:00' })
    expect(options[95]).toEqual({ value: 1425, label: '23:45' })
    expect(formatTimeMinutes(547)).toBe('09:07')
  })
})
