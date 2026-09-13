import {
  expectArray,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'

export const AGENT_GET_LOCAL_TOKEN_USAGE_METHOD = 'agent.getLocalTokenUsage'

export interface LocalTokenUsageSummaryInput {
  from: string
  to: string
}

export interface LocalTokenUsageDay {
  date: string
  tokenCount: string
}

export interface LocalTokenUsageSummaryOutput {
  timezone: 'Asia/Shanghai'
  startedAt: number
  days: LocalTokenUsageDay[]
  /** All-history aggregates, not restricted by the requested daily display window. */
  totalTokens: string
  todayTokens: string
  peakDailyTokens: string
  unreportedRequestCount: number
}

function date(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (
    !/^\d{4}-\d{2}-\d{2}$/.test(result) ||
    result < '1970-01-01' ||
    !Number.isFinite(Date.parse(`${result}T00:00:00Z`)) ||
    new Date(`${result}T00:00:00Z`).toISOString().slice(0, 10) !== result
  ) {
    throw invalidProtocolValue(context, 'expected a valid YYYY-MM-DD date from 1970 onward')
  }
  return result
}

function count(value: unknown, context: string): string {
  const result = expectString(value, context)
  if (!/^(0|[1-9]\d{0,38})$/.test(result) || BigInt(result) > (1n << 128n) - 1n) {
    throw invalidProtocolValue(context, 'expected a non-negative decimal u128 string')
  }
  return result
}

export function parseLocalTokenUsageSummaryInput(value: unknown): LocalTokenUsageSummaryInput {
  const record = expectRecord(value, 'Local token usage input')
  expectOnlyKeys(record, ['from', 'to'], 'Local token usage input')
  const from = date(record.from, 'from')
  const to = date(record.to, 'to')
  if (from > to || (Date.parse(to) - Date.parse(from)) / 86_400_000 >= 3660) {
    throw invalidProtocolValue(
      'Local token usage input',
      'expected an ordered range of at most 3660 days'
    )
  }
  return { from, to }
}

export function parseLocalTokenUsageSummaryOutput(value: unknown): LocalTokenUsageSummaryOutput {
  const record = expectRecord(value, 'Local token usage output')
  expectOnlyKeys(
    record,
    [
      'timezone',
      'startedAt',
      'days',
      'totalTokens',
      'todayTokens',
      'peakDailyTokens',
      'unreportedRequestCount'
    ],
    'Local token usage output'
  )
  const days = expectArray(record.days, 'days').map((value): LocalTokenUsageDay => {
    const row = expectRecord(value, 'day')
    expectOnlyKeys(row, ['date', 'tokenCount'], 'day')
    return { date: date(row.date, 'date'), tokenCount: count(row.tokenCount, 'tokenCount') }
  })
  if (
    days.length > 3660 ||
    days.some((day, index) => index > 0 && day.date <= days[index - 1].date)
  ) {
    throw invalidProtocolValue('days', 'expected at most 3660 unique days in ascending order')
  }
  return {
    timezone: expectEnum(record.timezone, ['Asia/Shanghai'], 'timezone'),
    startedAt: expectSafeInteger(record.startedAt, 'startedAt', 0),
    days,
    totalTokens: count(record.totalTokens, 'totalTokens'),
    todayTokens: count(record.todayTokens, 'todayTokens'),
    peakDailyTokens: count(record.peakDailyTokens, 'peakDailyTokens'),
    unreportedRequestCount: expectSafeInteger(
      record.unreportedRequestCount,
      'unreportedRequestCount',
      0
    )
  }
}
