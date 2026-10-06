import { describe, expect, it, vi } from 'vitest'
import { diagnosticMethod, RpcDiagnostics } from './rpcDiagnostics'

describe('RPC scalar diagnostics', () => {
  it('aggregates bounded labels and never retains dynamic method names', () => {
    const emit = vi.fn()
    const diagnostics = new RpcDiagnostics(true, emit)
    for (let i = 0; i < 1000; i += 1) {
      diagnostics.record('response.parse', `private-path-secret-${i}`, 2, 20)
    }
    diagnostics.record('response.parse', 'storage.loadConversation', 10, 5000)
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
        'response.parse:storage.loadConversation': { count: 1, bytes: 5000 }
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
