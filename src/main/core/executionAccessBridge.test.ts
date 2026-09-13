import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AuthState, LicenseState } from '@mycopilot/host-api'
import { ExecutionAccessBridge } from '../auth/ExecutionAccessBridge'
import { parseExecutionAccessSnapshot, parseExecutionAccessReceipt } from '@mycopilot/protocol'

const bridges: ExecutionAccessBridge[] = []
function setup(running = true) {
  let state = { status: 'signedOut', profile: null } as AuthState
  let onAuth!: (state: AuthState) => void
  let onLicense!: (state: LicenseState) => void
  let started!: () => void
  const lease = vi.fn(() => ({ reason: 'allowed' as const, remainingMs: 120_000 }))
  const core = {
    isRunning: vi.fn(() => running),
    onStarted: vi.fn((handler: () => void) => {
      started = handler
      return vi.fn()
    }),
    setExecutionAccess: vi.fn(async (snapshot) => ({ revision: snapshot.revision }))
  }
  const auth = {
    getState: () => state,
    subscribe: (handler: (state: AuthState) => void) => {
      onAuth = handler
      return vi.fn()
    }
  }
  const license = {
    getExecutionLease: lease,
    subscribe: (handler: (state: LicenseState) => void) => {
      onLicense = handler
      return vi.fn()
    }
  }
  const bridge = new ExecutionAccessBridge(auth, license, core)
  bridges.push(bridge)
  return {
    bridge,
    core,
    lease,
    start: () => {
      running = true
      started()
    },
    emitLicense: () => onLicense({} as LicenseState),
    account: (userId: string | null) => {
      state = {
        status: userId ? 'signedIn' : 'signedOut',
        profile: userId ? { userId } : null
      } as AuthState
      onAuth(state)
    }
  }
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-09-13T08:00:00Z'))
})
afterEach(() => {
  for (const bridge of bridges.splice(0)) bridge.dispose()
  vi.useRealTimers()
})

describe('local execution access mirroring', () => {
  it('sends default denial, bounded permission and immediate logout with increasing identity generations', async () => {
    const s = setup()
    expect(s.core.setExecutionAccess.mock.lastCall?.[0]).toMatchObject({
      reason: 'account_signed_out',
      identityEpoch: 0
    })
    s.account('a')
    const allowed = s.core.setExecutionAccess.mock.lastCall![0]
    expect(allowed).toMatchObject({ reason: 'allowed', identityEpoch: 1 })
    expect(allowed.validUntil - allowed.issuedAt).toBe(60_000)
    expect(Object.keys(allowed).sort()).toEqual([
      'identityEpoch',
      'issuedAt',
      'reason',
      'revision',
      'validUntil'
    ])
    s.account(null)
    const denied = s.core.setExecutionAccess.mock.lastCall![0]
    expect(denied).toMatchObject({ reason: 'account_signed_out', identityEpoch: 2 })
    expect(denied.validUntil).toBe(denied.issuedAt)
    expect(denied.revision).toBeGreaterThan(allowed.revision)
    s.account('b')
    expect(s.core.setExecutionAccess.mock.lastCall?.[0].identityEpoch).toBe(3)
  })
  it('only renews IPC every 30 seconds and caps it at the real license remainder', async () => {
    const s = setup()
    s.account('a')
    s.lease.mockReturnValue({ reason: 'allowed', remainingMs: 1_500 })
    s.emitLicense()
    let last = s.core.setExecutionAccess.mock.lastCall![0]
    expect(last.validUntil - last.issuedAt).toBe(1_500)
    s.lease.mockReturnValue({ reason: 'allowed', remainingMs: 0 })
    await vi.advanceTimersByTimeAsync(30_000)
    last = s.core.setExecutionAccess.mock.lastCall![0]
    expect(last.reason).toBe('license_unavailable')
    expect(last.validUntil).toBe(last.issuedAt)
  })
  it('resends after a lazy Core restart, but never starts a stopped Core or runs after disposal', async () => {
    const s = setup(false)
    s.account('a')
    await vi.advanceTimersByTimeAsync(30_000)
    expect(s.core.setExecutionAccess).not.toHaveBeenCalled()
    s.start()
    expect(s.core.setExecutionAccess).toHaveBeenCalledOnce()
    s.bridge.dispose()
    await vi.advanceTimersByTimeAsync(90_000)
    await s.bridge.sync()
    expect(s.core.setExecutionAccess).toHaveBeenCalledOnce()
  })
  it('reports failed acknowledgements without permitting caller runNow to continue', async () => {
    const s = setup(false)
    s.core.setExecutionAccess.mockResolvedValue({ revision: 0 })
    s.start()
    await expect(s.bridge.sync()).rejects.toThrow('not applied')
  })
})

describe('execution access private transport contract', () => {
  const valid = {
    revision: 1,
    identityEpoch: 0,
    reason: 'allowed',
    issuedAt: 1000,
    validUntil: 61_000
  }
  it('accepts only bounded credential-free snapshots and validated acknowledgements', () => {
    expect(parseExecutionAccessSnapshot(valid)).toEqual(valid)
    expect(parseExecutionAccessReceipt({ revision: 1 })).toEqual({ revision: 1 })
    expect(() => parseExecutionAccessReceipt({ revision: 0 })).toThrow()
    expect(() => parseExecutionAccessReceipt({ revision: 1, private: true })).toThrow()
  })
  it.each([
    { validUntil: 61_001 },
    { reason: 'license_required' },
    { revision: 0 },
    { identityEpoch: -1 },
    { token: 'not-allowed' },
    { validUntil: 1000 },
    { issuedAt: 1.5 }
  ])('rejects unsafe or inconsistent snapshots %o', (patch) => {
    expect(() => parseExecutionAccessSnapshot({ ...valid, ...patch })).toThrow()
  })
})
