import { describe, expect, it, vi } from 'vitest'
import { LatencyHistogram, LATENCY_BUCKETS_MS } from '../../shared/latencyHistogram'
import { diagnosticMethod, RpcDiagnostics } from './rpcDiagnostics'

describe('RPC scalar diagnostics', () => {
  it('aggregates bounded labels and never retains dynamic method names', () => {
    const emit = vi.fn()
    const diagnostics = new RpcDiagnostics(true, emit)
    for (let i = 0; i < 1000; i += 1) {
      diagnostics.record('response.parse', `private-path-secret-${i}`, 2, 20)
    }
    diagnostics.record('response.parse', 'storage.loadConversation', 10, 5000)
    diagnostics.record('response', 'agent.getUsageSummary', 12)
    diagnostics.record('response', 'agent.getUsageDashboard', 8, 1200)
    diagnostics.queues(5, 4096)
    diagnostics.queues(2, 64)
    diagnostics.flush()
    expect(emit).toHaveBeenCalledOnce()
    const summary = emit.mock.calls[0][0]
    expect(summary).toMatchObject({
      pendingPeak: 5,
      stdinBytesPeak: 4096,
      samples: {
        'response.parse:other': {
          count: 1000,
          totalMs: 2000,
          maxMs: 2,
          bytes: 20000,
          maxBytes: 20
        },
        'response.parse:storage.loadConversation': { count: 1, bytes: 5000 },
        'response:agent.getUsageSummary': { count: 1, totalMs: 12 },
        'response:agent.getUsageDashboard': { count: 1, totalMs: 8, bytes: 1200 }
      }
    })
    expect(JSON.stringify(summary)).not.toContain('private-path-secret')
    diagnostics.flush()
    expect(emit).toHaveBeenCalledOnce()
    diagnostics.record('response', 'core.ping', 1)
    diagnostics.flush()
    expect(emit.mock.calls[1][0]).toMatchObject({ pendingPeak: 0, stdinBytesPeak: 0 })
  })

  it('is silent while disabled and does not trust arbitrary labels', () => {
    const emit = vi.fn()
    const diagnostics = new RpcDiagnostics(false, emit)
    diagnostics.queues(100, 1_000_000)
    diagnostics.record('response', 'core.ping', 500)
    diagnostics.flush()
    expect(emit).not.toHaveBeenCalled()
    expect(diagnosticMethod({ content: 'secret' })).toBe('other')
    expect(diagnosticMethod('storage.loadConversation\nsecret')).toBe('other')
  })

  it('emits at most one aggregate per interval and resets the window', () => {
    const clock = vi.spyOn(performance, 'now').mockReturnValue(0)
    try {
      const emit = vi.fn()
      const diagnostics = new RpcDiagnostics(true, emit)
      diagnostics.record('response', 'core.ping', 1)
      clock.mockReturnValue(30_000)
      diagnostics.record('response', 'core.ping', 2)
      diagnostics.record('response', 'core.ping', 3)
      expect(emit).toHaveBeenCalledOnce()
      expect(emit.mock.calls[0][0].samples['response:core.ping'].count).toBe(2)
      diagnostics.flush()
      expect(emit.mock.calls[1][0].samples['response:core.ping'].count).toBe(1)
    } finally {
      clock.mockRestore()
    }
  })
})

describe('bounded latency distributions and RPC correlation', () => {
  it('retains tails as bucket bounds and provides mergeable counts', () => {
    const emit = vi.fn()
    const diagnostics = new RpcDiagnostics(true, emit)
    for (let i = 0; i < 100; i += 1) {
      diagnostics.record('response', 'agent.approveAction', i === 99 ? 900 : i === 98 ? 90 : 3)
    }
    diagnostics.flush()
    const histogram = emit.mock.calls[0][0].samples['response:agent.approveAction']
    expect(histogram).toMatchObject({
      count: 100,
      p50UpperMs: 4,
      p95UpperMs: 4,
      p99UpperMs: 125,
      maxMs: 900
    })
    expect(histogram.bucketCounts.reduce((sum: number, count: number) => sum + count, 0)).toBe(100)
    expect(emit.mock.calls[0][0].percentileKind).toBe('fixed_bucket_upper_bound')
  })

  it('keeps queue gauges and outcomes without cardinality growth from request IDs', () => {
    const emit = vi.fn()
    const diagnostics = new RpcDiagnostics(true, emit)
    for (let id = 1; id <= 100; id += 1) {
      diagnostics.requestOutcome('offered', 'secret-method', id)
      diagnostics.requestOutcome('completed', 'secret-method', id, 3)
    }
    diagnostics.requestOutcome('rpc_error', 'agent.approveAction', 101, 50)
    diagnostics.requestOutcome('transport_failed', 'agent.cancelRun', 102, 60)
    diagnostics.queues(10, 1000, 800)
    diagnostics.queues(2, 25, 20)
    diagnostics.flush()
    const summary = emit.mock.calls[0][0]
    expect(summary).toMatchObject({
      pendingPeak: 10,
      pendingCurrent: 2,
      stdinBytesPeak: 1000,
      stdinBytesCurrent: 25,
      oldestPendingMsPeak: 800,
      outcomes: {
        'offered:other': 100,
        'completed:other': 100,
        'rpc_error:agent.approveAction': 1,
        'transport_failed:agent.cancelRun': 1
      },
      omittedTraces: 70
    })
    expect(summary.traces).toHaveLength(32)
    expect(summary.traces[0]).toEqual({
      rpcId: 1,
      method: 'other',
      elapsedMs: 3,
      outcome: 'completed'
    })
    expect(JSON.stringify(summary)).not.toContain('secret-method')
  })
  it('defines overflow and inclusive bucket boundaries without accepting invalid durations', () => {
    const histogram = new LatencyHistogram()
    for (const invalid of [-1, NaN, Infinity]) histogram.record(invalid)
    histogram.record(0)
    histogram.record(0.05)
    histogram.record(0.051)
    histogram.record(2_000_000)
    const summary = histogram.snapshot()
    expect(summary).toMatchObject({
      count: 4,
      p50UpperMs: 0.05,
      p95UpperMs: 2_000_000,
      p99UpperMs: 2_000_000,
      overflowCount: 1
    })
    expect(summary.bucketCounts).toHaveLength(LATENCY_BUCKETS_MS.length + 1)
    expect(summary.bucketCounts[0]).toBe(2)
    expect(summary.bucketCounts[1]).toBe(1)
    expect(summary.bucketCounts.at(-1)).toBe(1)
  })
})
