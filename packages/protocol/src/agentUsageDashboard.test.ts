import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  parseAgentUsageDashboardInput,
  parseAgentUsageDashboardOutput
} from './agentUsageDashboard'

const fixture = JSON.parse(
  readFileSync(resolve(process.cwd(), 'packages/protocol/fixtures/usage-dashboard-v1.json'), 'utf8')
)
const day = 86_400_000

describe('usage dashboard contract', () => {
  it('preserves ordered buckets, manual-compaction counts, and unknown optional counters in the Rust fixture', () => {
    const input = parseAgentUsageDashboardInput(fixture.input)
    expect(input).toEqual(fixture.input)
    const output = parseAgentUsageDashboardOutput(fixture.output, input.windows.length)
    expect(output).toEqual(fixture.output)
    expect(output.summary.messageCount).toBe(1)
    expect(output.summary.requestCount).toBe(2)
    expect(output.buckets[0].requestCount).toBe(1)
    expect(output.buckets[0].messageCount).toBe(0)
    expect(output.buckets[1].messageCount).toBe(1)
    expect(output.buckets[1]).not.toHaveProperty('estimatedCost')
    expect(output.summary).not.toHaveProperty('outputThinkingTokens')
  })

  it('admits inclusive bounded windows and rejects gaps, overlaps and unsafe timestamps', () => {
    expect(parseAgentUsageDashboardInput({ windows: [{ from: 0, to: 370 * day - 1 }] })).toEqual({
      windows: [{ from: 0, to: 370 * day - 1 }]
    })
    const windows = Array.from({ length: 31 }, (_, index) => ({ from: index, to: index }))
    expect(parseAgentUsageDashboardInput({ windows }).windows).toHaveLength(31)
    for (const input of [
      { windows: [] },
      { windows: [...windows, { from: 31, to: 31 }] },
      { windows: [{ from: 0, to: 370 * day }] },
      { windows: [{ from: 2, to: 1 }] },
      { windows: [{ from: -1, to: 1 }] },
      { windows: [{ from: 0.5, to: 1 }] },
      { windows: [{ from: 0, to: Number.MAX_SAFE_INTEGER + 1 }] },
      { windows: [{ from: 0, to: Infinity }] },
      {
        windows: [
          { from: 0, to: 1 },
          { from: 1, to: 2 }
        ]
      },
      {
        windows: [
          { from: 0, to: 1 },
          { from: 3, to: 4 }
        ]
      },
      { windows: [{ from: 0, to: 1, timezone: 'UTC' }] },
      { windows: [{ from: 0, to: 1 }], modelId: 'model-1' }
    ]) {
      expect(() => parseAgentUsageDashboardInput(input)).toThrow()
    }
  })

  it('rejects mismatched or malformed dashboard responses before chart indexing', () => {
    expect(() => parseAgentUsageDashboardOutput(fixture.output, 3)).toThrow()
    for (const output of [
      { ...fixture.output, buckets: [] },
      { ...fixture.output, buckets: Array.from({ length: 32 }, () => fixture.output.summary) },
      { ...fixture.output, summary: { ...fixture.output.summary, requestCount: -1 } },
      { ...fixture.output, summary: { ...fixture.output.summary, estimatedCost: Infinity } },
      { ...fixture.output, summary: { ...fixture.output.summary, inputTokens: null } },
      { ...fixture.output, rawRecords: [] }
    ]) {
      expect(() => parseAgentUsageDashboardOutput(output)).toThrow()
    }
  })
})
