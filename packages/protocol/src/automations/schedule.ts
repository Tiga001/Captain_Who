import {
  expectArray,
  expectEnum,
  expectRecord,
  expectSafeInteger,
  expectOnlyKeys,
  invalidProtocolValue
} from '../skills/validation'
import { intInRange, boundedString, ZONE_MAX } from './validation'
import type { AutomationScheduleInput } from './types'

const WEEKDAYS = [
  'monday',
  'tuesday',
  'wednesday',
  'thursday',
  'friday',
  'saturday',
  'sunday'
] as const

function parseStringArrayEnum<T extends readonly string[]>(
  value: unknown,
  values: T,
  context: string
): T[number][] {
  const array = expectArray(value, context)
  return array.map((item, index) => expectEnum(item, values, `${context}[${index}]`))
}

function parseIntegerArray(value: unknown, context: string, min: number, max: number): number[] {
  return expectArray(value, context).map((item, index) =>
    intInRange(item, `${context}[${index}]`, min, max)
  )
}

export function parseAutomationScheduleInput(value: unknown): AutomationScheduleInput {
  const context = 'Automation schedule'
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['interval', 'daily', 'weekdays', 'weekly', 'custom'] as const,
    `${context}.kind`
  )
  const common = {
    anchorAt: expectSafeInteger(record.anchorAt, `${context}.anchorAt`, 0),
    timezone: boundedString(record.timezone, `${context}.timezone`, ZONE_MAX)
  }
  if (kind === 'interval') {
    expectOnlyKeys(record, ['kind', 'amount', 'unit', 'anchorAt', 'timezone'], context)
    return {
      kind,
      amount: expectSafeInteger(record.amount, `${context}.amount`, 1),
      unit: expectEnum(record.unit, ['minutes', 'hours', 'days'] as const, `${context}.unit`),
      ...common
    }
  }
  if (kind === 'daily' || kind === 'weekdays') {
    expectOnlyKeys(record, ['kind', 'timeMinutes', 'anchorAt', 'timezone'], context)
    return {
      kind,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  if (kind === 'weekly') {
    expectOnlyKeys(record, ['kind', 'weekdays', 'timeMinutes', 'anchorAt', 'timezone'], context)
    return {
      kind,
      weekdays: parseStringArrayEnum(record.weekdays, WEEKDAYS, `${context}.weekdays`),
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  expectOnlyKeys(
    record,
    [
      'kind',
      'frequency',
      'interval',
      'minuteOfHour',
      'timeMinutes',
      'weekdays',
      'monthDays',
      'months',
      'anchorAt',
      'timezone'
    ],
    context
  )
  const frequency = expectEnum(
    record.frequency,
    ['hourly', 'daily', 'weekly', 'monthly', 'yearly'] as const,
    `${context}.frequency`
  )
  const interval = expectSafeInteger(record.interval, `${context}.interval`, 1)
  const present = (field: string): boolean => record[field] !== undefined && record[field] !== null
  const reject = (fields: string[]): void => {
    const unexpected = fields.find(present)
    if (unexpected !== undefined)
      throw invalidProtocolValue(`${context}.${unexpected}`, `is not valid for ${frequency}`)
  }
  if (frequency === 'hourly') {
    reject(['timeMinutes', 'weekdays', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      minuteOfHour: intInRange(record.minuteOfHour, `${context}.minuteOfHour`, 0, 59),
      ...common
    }
  }
  if (frequency === 'daily') {
    reject(['minuteOfHour', 'weekdays', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      ...common
    }
  }
  if (frequency === 'weekly') {
    reject(['minuteOfHour', 'monthDays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      weekdays: parseStringArrayEnum(record.weekdays, WEEKDAYS, `${context}.weekdays`),
      ...common
    }
  }
  if (frequency === 'monthly') {
    reject(['minuteOfHour', 'weekdays', 'months'])
    return {
      kind,
      frequency,
      interval,
      timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
      monthDays: parseIntegerArray(record.monthDays, `${context}.monthDays`, 1, 31),
      ...common
    }
  }
  reject(['minuteOfHour', 'weekdays'])
  return {
    kind,
    frequency,
    interval,
    timeMinutes: intInRange(record.timeMinutes, `${context}.timeMinutes`, 0, 1439),
    monthDays: parseIntegerArray(record.monthDays, `${context}.monthDays`, 1, 31),
    months: parseIntegerArray(record.months, `${context}.months`, 1, 12),
    ...common
  }
}
