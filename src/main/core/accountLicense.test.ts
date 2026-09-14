import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AuthState } from '@mycopilot/host-api'
import { LicenseService } from '../auth/LicenseService'
import {
  LicenseFailure,
  LICENSE_CACHE_MS,
  parseVerifiedLicense,
  type VerifiedLicense
} from '../auth/LicenseApiClient'

const services: LicenseService[] = []
const base = Date.parse('2026-09-13T06:00:00.000Z')
const grant = (ttl = LICENSE_CACHE_MS): VerifiedLicense => ({
  allowed: true,
  reason: 'active',
  expiresAt: null,
  verifiedAt: new Date(Date.now()).toISOString(),
  cacheValidUntil: new Date(Date.now() + ttl).toISOString()
})
function setup(clock?: { wall: number; mono: number }) {
  let authState = { status: 'signedOut', profile: null } as AuthState
  let listener: ((state: AuthState) => void) | undefined
  const auth = {
    getState: () => authState,
    subscribe: (next: (state: AuthState) => void) => {
      listener = next
      return () => {
        listener = undefined
      }
    },
    getAccessToken: vi.fn((id: string) => `token-for-${id}`),
    refreshProfile: vi.fn().mockResolvedValue({ ok: true })
  }
  const fetch = vi.fn().mockImplementation(async () => grant())
  const service = new LicenseService(
    auth,
    fetch,
    clock ? () => clock.wall : undefined,
    clock ? () => clock.mono : undefined
  )
  services.push(service)
  const account = (userId: string | null): void => {
    authState = {
      status: userId ? 'signedIn' : 'signedOut',
      profile: userId ? { userId } : null
    } as AuthState
    listener?.(authState)
  }
  return { service, fetch, auth, account }
}
beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(base)
})
afterEach(() => {
  for (const service of services.splice(0)) service.dispose()
  vi.useRealTimers()
})

describe('24-hour account license admission', () => {
  it('does not query while signed out and deduplicates initial requests', async () => {
    const s = setup()
    expect(s.service.getState().status).toBe('signedOut')
    expect(s.fetch).not.toHaveBeenCalled()
    s.account('a')
    await Promise.all([s.service.refresh(), s.service.refresh()])
    expect(s.fetch).toHaveBeenCalledTimes(1)
    expect(() => s.service.assertCanStartTurn()).not.toThrow()
    await vi.advanceTimersByTimeAsync(23 * 60 * 60_000)
    s.account('a') // periodic account validation is not a license heartbeat
    expect(s.service.getState().status).toBe('allowed')
    expect(s.fetch).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(60 * 60_000)
    expect(s.fetch).toHaveBeenCalledTimes(2)
  })
  it('requires fresh online permission after restart, even within the prior 24-hour window', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    s.service.dispose()
    vi.setSystemTime(base + 12 * 60 * 60_000)
    const next = setup()
    next.account('a')
    expect(next.service.getExecutionLease().reason).not.toBe('allowed')
    expect(next.fetch).toHaveBeenCalledTimes(1)
    await next.service.refresh()
    expect(next.service.getState().cacheValidUntil).toBe(
      new Date(base + 12 * 60 * 60_000 + LICENSE_CACHE_MS).toISOString()
    )
    expect(next.service.getExecutionLease().reason).toBe('allowed')
    next.account('b')
    await next.service.refresh()
    expect(next.fetch).toHaveBeenCalledWith('token-for-b', expect.any(AbortSignal))
    next.account('a')
    expect(next.service.getExecutionLease().reason).not.toBe('allowed')
    await next.service.refresh()
    expect(next.fetch).toHaveBeenCalledTimes(3)
  })
  it('never grants another account a late response after logout', async () => {
    const s = setup()
    let complete!: (value: VerifiedLicense) => void
    s.fetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve
        })
    )
    s.account('a')
    const pending = s.service.refresh()
    s.account(null)
    complete(grant())
    await pending
    expect(s.service.getState().status).toBe('signedOut')
    expect(() => s.service.assertCanStartTurn()).toThrow('ACCOUNT_LOGIN_REQUIRED')
  })
  it('caps a cached grant at its actual license expiry and blocks only admission offline', async () => {
    const s = setup()
    const license = grant(60_000)
    license.expiresAt = license.cacheValidUntil
    s.fetch.mockResolvedValueOnce(license).mockRejectedValue(new LicenseFailure('network'))
    s.account('a')
    await s.service.refresh()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(() => s.service.assertCanStartTurn()).toThrow('ACCOUNT_LICENSE_UNAVAILABLE')
    expect(s.service.getState().status).toBe('unavailable')
    // Service has no run/cancel/shutdown dependencies; expiry cannot stop existing work.
    expect(s.fetch).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(59_000)
    expect(s.fetch).toHaveBeenCalledTimes(2)
  })
  it('retains a still-valid grant after a transient manual check failure', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    s.fetch.mockRejectedValue(new LicenseFailure('network'))
    await s.service.refresh()
    expect(s.service.getState()).toMatchObject({ status: 'allowed', error: 'network' })
    expect(() => s.service.assertCanStartTurn()).not.toThrow()
  })
  it('does not treat missing license provisioning as default permission', async () => {
    const s = setup()
    s.fetch.mockRejectedValue(new LicenseFailure('notProvisioned'))
    s.account('a')
    await s.service.refresh()
    expect(s.service.getState()).toMatchObject({ status: 'unavailable', error: 'notProvisioned' })
    expect(() => s.service.assertCanStartTurn()).toThrow()
  })
  it('invalidates an earlier grant when the server explicitly reports it is no longer provisioned', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    expect(s.service.getExecutionLease().reason).toBe('allowed')
    const states: Array<{ reason: string | null }> = []
    s.service.subscribe((state) => states.push(state))
    s.fetch.mockRejectedValue(new LicenseFailure('notProvisioned'))
    await s.service.refresh()
    expect(s.service.getState()).toMatchObject({
      status: 'unavailable',
      error: 'notProvisioned',
      reason: null
    })
    expect(states.some((state) => state.reason === 'expired')).toBe(false)
    expect(s.service.getExecutionLease().reason).toBe('license_required')
  })
  it('rejects a revoked grant and cannot restore former permission offline', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    s.fetch.mockResolvedValue({
      ...grant(),
      allowed: false,
      reason: 'revoked',
      cacheValidUntil: new Date(Date.now()).toISOString()
    })
    await s.service.refresh()
    expect(s.service.getState()).toMatchObject({ status: 'denied', reason: 'revoked' })
    expect(() => s.service.assertCanStartTurn()).toThrow()
    s.service.dispose()
    const restarted = setup()
    restarted.fetch.mockRejectedValue(new LicenseFailure('network'))
    restarted.account('a')
    await restarted.service.refresh()
    expect(restarted.service.getExecutionLease().reason).not.toBe('allowed')
  })
  it('cannot extend an old permission by repeatedly restarting with a rolled-back clock', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    expect(s.service.getExecutionLease().reason).toBe('allowed')
    s.service.dispose()
    for (let restart = 0; restart < 30; restart++) {
      vi.setSystemTime(base + 1000)
      const next = setup()
      next.fetch.mockRejectedValue(new LicenseFailure('network'))
      next.account('a')
      await next.service.refresh()
      expect(next.fetch).toHaveBeenCalledTimes(1)
      expect(next.service.getExecutionLease().reason).not.toBe('allowed')
      next.service.dispose()
    }
  })
  it('refreshes authentication once on an unauthorized response', async () => {
    const s = setup()
    s.fetch
      .mockRejectedValueOnce(new LicenseFailure('network', true))
      .mockResolvedValueOnce(grant())
    s.account('a')
    await s.service.refresh()
    expect(s.auth.refreshProfile).toHaveBeenCalledOnce()
    expect(s.fetch).toHaveBeenCalledTimes(2)
    expect(s.service.getState().status).toBe('allowed')
  })
  it('uses a monotonic deadline if the local wall clock stops moving', async () => {
    const clock = { wall: base, mono: 0 }
    const s = setup(clock)
    s.account('a')
    await s.service.refresh()
    s.fetch.mockRejectedValue(new LicenseFailure('network'))
    clock.mono += LICENSE_CACHE_MS
    expect(s.service.getExecutionLease().reason).not.toBe('allowed')
    await s.service.refresh()
    expect(s.service.getState().status).toBe('unavailable')
  })
  it('never revives a grant after observing a clock rollback then correcting it offline', async () => {
    const clock = { wall: base, mono: 0 }
    const s = setup(clock)
    s.account('a')
    await s.service.refresh()
    s.fetch.mockRejectedValue(new LicenseFailure('network'))
    clock.wall = base - 1
    expect(s.service.getExecutionLease().reason).not.toBe('allowed')
    clock.wall = base + 1000
    await s.service.refresh()
    expect(s.service.getExecutionLease().reason).not.toBe('allowed')
  })
  it('subtracts in-flight request latency from the process-local grant', async () => {
    const clock = { wall: base, mono: 0 }
    const s = setup(clock)
    const result = grant()
    let complete!: (value: VerifiedLicense) => void
    s.fetch.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve
        })
    )
    s.account('a')
    const pending = s.service.refresh()
    clock.wall += 60_000
    clock.mono += 60_000
    complete(result)
    await pending
    expect(s.service.getExecutionLease()).toEqual({
      reason: 'allowed',
      remainingMs: LICENSE_CACHE_MS - 60_000
    })
  })
})

describe('license API contract', () => {
  it.each([
    { cacheValidUntil: new Date(base + LICENSE_CACHE_MS + 1).toISOString() },
    { expiresAt: new Date(base + 60_000).toISOString() },
    { reason: 'revoked' },
    { expiresAt: undefined },
    { allowed: 'true' },
    { verifiedAt: 'invalid' }
  ])('rejects malformed or overlong grants', (value) => {
    expect(() => parseVerifiedLicense({ ...grant(), ...value })).toThrow('invalidResponse')
  })
})
