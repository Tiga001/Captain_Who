import { describe, expect, it } from 'vitest'
import {
  parseLocalTokenUsageSummaryInput,
  parseLocalTokenUsageSummaryOutput
} from './localTokenUsage'

const summary = {
  timezone: 'Asia/Shanghai',
  startedAt: 1,
  days: [{ date: '2026-09-13', tokenCount: '9007199254740993' }],
  totalTokens: '9007199254740993',
  todayTokens: '0',
  peakDailyTokens: '9007199254740993',
  unreportedRequestCount: 2
}

describe('local token usage protocol', () => {
  it('preserves large decimal integers without floating point conversion', () => {
    expect(parseLocalTokenUsageSummaryOutput(summary)).toEqual(summary)
  })
  it('validates calendar dates and bounded ordered ranges', () => {
    expect(parseLocalTokenUsageSummaryInput({ from: '2024-02-29', to: '2024-03-01' })).toEqual({
      from: '2024-02-29',
      to: '2024-03-01'
    })
    for (const input of [
      { from: '2026-02-29', to: '2026-03-01' },
      { from: '2026-03-02', to: '2026-03-01' },
      { from: '2020-01-01', to: '2040-01-01' },
      { from: '2026-01-01', to: '2026-01-01', accountId: 'not-an-account-statistic' }
    ])
      expect(() => parseLocalTokenUsageSummaryInput(input)).toThrow()
  })
  it('rejects extra information, invalid counts and duplicate days', () => {
    for (const value of [
      { ...summary, model: 'must-not-be-present' },
      { ...summary, totalTokens: 2 },
      { ...summary, totalTokens: '-1' },
      { ...summary, totalTokens: '01' },
      { ...summary, totalTokens: (1n << 128n).toString() },
      { ...summary, days: [...summary.days, ...summary.days] },
      { ...summary, unreportedRequestCount: 1.5 }
    ])
      expect(() => parseLocalTokenUsageSummaryOutput(value)).toThrow()
  })
})
