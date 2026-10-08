import type {
  AgentUsageDashboardInput,
  AgentUsageDashboardOutput,
  AgentUsageModelSummary,
  AgentUsageSummaryOutput
} from './agent/conversation'
import {
  expectArray,
  expectBoolean,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'

const MAX_WINDOWS = 31
const MAX_SPAN_MS = 370 * 24 * 60 * 60 * 1000
const TOKEN_KEYS = [
  'inputTokens',
  'outputTokens',
  'outputThinkingTokens',
  'totalTokens',
  'cachedInputTokens',
  'cacheCreationInputTokens'
] as const
const TOTAL_KEYS = [
  'requestCount',
  'messageCount',
  'unpricedMessageCount',
  ...TOKEN_KEYS,
  'estimatedCost'
]

export function parseAgentUsageDashboardInput(value: unknown): AgentUsageDashboardInput {
  const record = expectRecord(value, 'usage dashboard input')
  expectOnlyKeys(record, ['windows'], 'usage dashboard input')
  const rawWindows = expectArray(record.windows, 'usage windows')
  if (rawWindows.length < 1 || rawWindows.length > MAX_WINDOWS) {
    throw invalidProtocolValue('usage windows', 'expected between 1 and 31 windows')
  }
  const windows = rawWindows.map((value) => {
    const window = expectRecord(value, 'usage window')
    expectOnlyKeys(window, ['from', 'to'], 'usage window')
    const from = expectSafeInteger(window.from, 'usage window from', 0)
    const to = expectSafeInteger(window.to, 'usage window to', 0)
    if (from > to) throw invalidProtocolValue('usage window', 'expected ordered inclusive bounds')
    return { from, to }
  })
  if (
    windows.some((window, index) => index > 0 && window.from !== windows[index - 1].to + 1) ||
    windows[windows.length - 1].to - windows[0].from + 1 > MAX_SPAN_MS
  ) {
    throw invalidProtocolValue(
      'usage windows',
      'expected contiguous windows covering at most 370 days'
    )
  }
  return { windows }
}

function count(value: unknown, context: string): number {
  // Existing usage aggregates are u64 JSON numbers. Preserve their legacy number representation;
  // safe-integer enforcement belongs to the new timestamp input, not historical usage counters.
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 0) {
    throw invalidProtocolValue(context, 'expected a non-negative integer')
  }
  return value
}

function totals(record: Record<string, unknown>): Omit<AgentUsageSummaryOutput, 'models'> {
  const result: Omit<AgentUsageSummaryOutput, 'models'> = {
    requestCount: count(record.requestCount, 'usage requestCount'),
    messageCount: count(record.messageCount, 'usage messageCount'),
    unpricedMessageCount: count(record.unpricedMessageCount, 'usage unpricedMessageCount')
  }
  for (const key of TOKEN_KEYS) {
    if (record[key] !== undefined) result[key] = count(record[key], `usage ${key}`)
  }
  if (record.estimatedCost !== undefined) {
    if (
      typeof record.estimatedCost !== 'number' ||
      !Number.isFinite(record.estimatedCost) ||
      record.estimatedCost < 0
    ) {
      throw invalidProtocolValue('usage estimatedCost', 'expected a non-negative finite number')
    }
    result.estimatedCost = record.estimatedCost
  }
  return result
}

function summary(value: unknown): AgentUsageSummaryOutput {
  const record = expectRecord(value, 'usage summary')
  expectOnlyKeys(record, [...TOTAL_KEYS, 'models'], 'usage summary')
  const models = expectArray(record.models, 'usage models').map((value): AgentUsageModelSummary => {
    const model = expectRecord(value, 'usage model')
    expectOnlyKeys(model, [...TOTAL_KEYS, 'modelId', 'modelName', 'isConfigured'], 'usage model')
    return {
      ...totals(model),
      modelId: expectString(model.modelId, 'usage modelId'),
      modelName: expectString(model.modelName, 'usage modelName'),
      isConfigured: expectBoolean(model.isConfigured, 'usage isConfigured')
    }
  })
  return { ...totals(record), models }
}

export function parseAgentUsageDashboardOutput(
  value: unknown,
  expectedBucketCount?: number
): AgentUsageDashboardOutput {
  const record = expectRecord(value, 'usage dashboard output')
  expectOnlyKeys(record, ['summary', 'buckets'], 'usage dashboard output')
  const buckets = expectArray(record.buckets, 'usage buckets')
  if (
    buckets.length < 1 ||
    buckets.length > MAX_WINDOWS ||
    (expectedBucketCount !== undefined && buckets.length !== expectedBucketCount)
  ) {
    throw invalidProtocolValue('usage buckets', 'expected one bucket for each requested window')
  }
  return { summary: summary(record.summary), buckets: buckets.map(summary) }
}
