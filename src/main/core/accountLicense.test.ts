import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import type { AuthState } from '@mycopilot/host-api'
import { LicenseService } from '../auth/LicenseService'
import {
  LicenseFailure,
  LICENSE_CACHE_MS,
  parseVerifiedLicense,
  type VerifiedLicense
} from '../auth/LicenseApiClient'
import { LicenseCacheStore, type CachedLicense } from '../auth/LicenseCacheStore'

const services: LicenseService[] = []
const base = Date.parse('2026-09-13T06:00:00.000Z')
const grant = (ttl = LICENSE_CACHE_MS): VerifiedLicense => ({
  allowed: true,
  reason: 'active',
  expiresAt: null,
  verifiedAt: new Date(Date.now()).toISOString(),
  cacheValidUntil: new Date(Date.now() + ttl).toISOString()
})
function setup(initial = new Map<string, CachedLicense>()) {
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
  const store = {
    read: vi.fn((id: string) => initial.get(id) ?? null),
    write: vi.fn((entry: CachedLicense) => {
      initial.set(entry.userId, structuredClone(entry))
      return true
    })
  }
  const service = new LicenseService(auth, store, fetch)
  services.push(service)
  const account = (userId: string | null): void => {
    authState = {
      status: userId ? 'signedIn' : 'signedOut',
      profile: userId ? { userId } : null
    } as AuthState
    listener?.(authState)
  }
  return { service, fetch, store, auth, account, initial }
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
  it('restores an account-bound cache across restart without renewing its deadline', async () => {
    const s = setup()
    s.account('a')
    await s.service.refresh()
    s.service.dispose()
    vi.setSystemTime(base + 12 * 60 * 60_000)
    const next = setup(s.initial)
    next.account('a')
    expect(next.service.getState().cacheValidUntil).toBe(
      new Date(base + LICENSE_CACHE_MS).toISOString()
    )
    expect(next.fetch).not.toHaveBeenCalled()
    next.account('b')
    await next.service.refresh()
    expect(next.fetch).toHaveBeenCalledWith('token-for-b', expect.any(AbortSignal))
    next.account('a')
    expect(next.fetch).toHaveBeenCalledTimes(1)
  })
  it('never grants another account a late response or writes it after logout', async () => {
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
    expect(s.store.write).not.toHaveBeenCalled()
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
  it('rejects a revoked grant and overwrites its former cached permission', async () => {
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
    expect(s.initial.get('a')?.license.allowed).toBe(false)
    expect(() => s.service.assertCanStartTurn()).toThrow()
  })
  it('rejects a saved cache from a future local clock', async () => {
    const initial = new Map([['a', { userId: 'a', receivedAt: base + 60_000, license: grant() }]])
    const s = setup(initial)
    s.fetch.mockRejectedValue(new LicenseFailure('network'))
    s.account('a')
    await s.service.refresh()
    expect(s.service.getState().status).toBe('unavailable')
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
  it('permits a verified memory-only lease when secure cache writing fails', async () => {
    const s = setup()
    s.store.write.mockReturnValue(false)
    s.account('a')
    await s.service.refresh()
    expect(s.service.getState()).toMatchObject({ status: 'allowed', error: 'storage' })
  })
})

describe('license API contract and encrypted cache', () => {
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
  it('stores only encrypted per-account results, isolated by environment', () => {
    const directory = mkdtempSync(join(tmpdir(), 'captain-license-test-'))
    const xor = (b: Buffer) => Buffer.from(b.map((v) => v ^ 0xaa))
    const encryption = {
      isEncryptionAvailable: () => true,
      encryptString: (v: string) => xor(Buffer.from(v)),
      decryptString: (v: Buffer) => xor(v).toString()
    }
    try {
      const cache = new LicenseCacheStore(directory, encryption, 'prod')
      const entry = { userId: 'test-user-a', receivedAt: Date.now(), license: grant() }
      expect(cache.write(entry)).toBe(true)
      expect(cache.read('test-user-a')).toEqual(entry)
      expect(cache.read('test-user-b')).toBeNull()
      expect(new LicenseCacheStore(directory, encryption, 'staging').read('test-user-a')).toBeNull()
      expect(
        readFileSync(join(directory, 'account-license.enc')).includes(Buffer.from('test-user-a'))
      ).toBe(false)
    } finally {
      rmSync(directory, { recursive: true })
    }
  })
})
