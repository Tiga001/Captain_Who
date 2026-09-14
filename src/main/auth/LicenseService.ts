import type { AuthState, LicenseState } from '@mycopilot/host-api'
import type { AuthService } from './AuthService'
import type { ExecutionAccessReason } from '@mycopilot/protocol'
import { LicenseFailure, parseVerifiedLicense, type VerifiedLicense } from './LicenseApiClient'

type Auth = Pick<AuthService, 'getState' | 'subscribe' | 'getAccessToken' | 'refreshProfile'>
type FetchLicense = (accessToken: string, signal: AbortSignal) => Promise<VerifiedLicense>
interface MemoryLicense {
  userId: string
  receivedAt: number
  license: VerifiedLicense
}
const EMPTY: Omit<LicenseState, 'revision'> = {
  status: 'signedOut',
  reason: null,
  expiresAt: null,
  verifiedAt: null,
  cacheValidUntil: null,
  error: null
}

/**
 * Account-specific, process-local admission. Only a fresh authenticated HTTPS response can
 * create a grant; on-disk data is never an authorization source, even after a restart.
 * Never owns a run, Core Server, or shutdown lifecycle.
 */
export class LicenseService {
  private state: LicenseState = { revision: 0, ...EMPTY }
  private userId: string | null = null
  private epoch = 0
  private lease: MemoryLicense | null = null
  private monotonicDeadline = 0
  private timer: ReturnType<typeof setTimeout> | undefined
  private controller: AbortController | null = null
  private pending: Promise<LicenseState> | null = null
  private retryAt = 0
  private failures = 0
  private listeners = new Set<(state: LicenseState) => void>()
  private unsubscribe: () => void

  constructor(
    private readonly auth: Auth,
    private readonly fetchLicense: FetchLicense,
    private readonly now: () => number = Date.now,
    private readonly monotonic: () => number = () => performance.now()
  ) {
    this.unsubscribe = auth.subscribe(this.accountChanged)
    this.accountChanged(auth.getState())
  }

  private publish(patch: Partial<LicenseState>): void {
    this.state = { ...this.state, ...patch, revision: this.state.revision + 1 }
    for (const listener of this.listeners) listener(structuredClone(this.state))
  }
  subscribe(listener: (state: LicenseState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }
  private accountChanged = (auth: AuthState): void => {
    const next = auth.status === 'signedIn' ? (auth.profile?.userId ?? null) : null
    if (next === this.userId) return
    ++this.epoch
    this.controller?.abort()
    this.controller = null
    this.pending = null
    this.clearTimer()
    this.userId = next
    this.lease = null
    this.retryAt = 0
    this.failures = 0
    this.publish({ ...EMPTY, status: next ? 'checking' : 'signedOut' })
    if (next) void this.refresh()
  }
  private clearTimer(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = undefined
  }
  private schedule(delay: number): void {
    this.clearTimer()
    this.timer = setTimeout(
      () => {
        this.expire()
        if (this.now() < this.retryAt) this.schedule(this.retryAt - this.now())
        else void this.refresh()
      },
      Math.max(1, delay)
    )
    this.timer.unref?.()
  }
  private deadline(entry: MemoryLicense): number {
    return (
      entry.receivedAt +
      Date.parse(entry.license.cacheValidUntil) -
      Date.parse(entry.license.verifiedAt)
    )
  }
  private valid(): boolean {
    return (
      !!this.lease &&
      this.lease.userId === this.userId &&
      this.lease.license.allowed &&
      this.now() >= this.lease.receivedAt &&
      this.now() < this.deadline(this.lease) &&
      this.monotonic() < this.monotonicDeadline
    )
  }
  private acceptVerified(entry: MemoryLicense): boolean {
    if (entry.userId !== this.userId || !Number.isFinite(entry.receivedAt)) return false
    let license: VerifiedLicense
    try {
      license = parseVerifiedLicense(entry.license)
    } catch {
      return false
    }
    const cached = { ...entry, license }
    const remaining = this.deadline(cached) - this.now()
    if (!license.allowed || entry.receivedAt > this.now() || remaining <= 0) return false
    this.lease = cached
    this.monotonicDeadline = this.monotonic() + remaining
    this.publish({
      status: 'allowed',
      reason: license.reason,
      expiresAt: license.expiresAt,
      verifiedAt: license.verifiedAt,
      cacheValidUntil: license.cacheValidUntil,
      error: null
    })
    this.schedule(remaining)
    return true
  }
  private expire(): void {
    if (this.state.status !== 'allowed' || this.valid()) return
    const knownExpiry =
      !!this.lease && this.lease.license.expiresAt === this.lease.license.cacheValidUntil
    // Once expiry/clock rollback is observed, correcting the clock cannot revive this grant.
    this.lease = null
    this.monotonicDeadline = 0
    this.publish({
      status: knownExpiry ? 'denied' : 'unavailable',
      reason: knownExpiry ? 'expired' : null
    })
  }
  getState = (): LicenseState => {
    this.expire()
    if (
      this.userId &&
      !this.valid() &&
      !this.pending &&
      this.state.status !== 'denied' &&
      this.now() >= this.retryAt
    )
      void this.refresh()
    return structuredClone(this.state)
  }
  assertCanStartTurn(): void {
    const { reason } = this.getExecutionLease()
    if (reason !== 'allowed') {
      if (this.userId && !this.pending && this.now() >= this.retryAt) void this.refresh()
      throw new Error(
        reason === 'account_signed_out'
          ? 'ACCOUNT_LOGIN_REQUIRED'
          : reason === 'license_required'
            ? 'ACCOUNT_LICENSE_REQUIRED'
            : 'ACCOUNT_LICENSE_UNAVAILABLE'
      )
    }
  }
  /** Main-only bounded authority for the local scheduler; never contains a user or token. */
  getExecutionLease(): { reason: ExecutionAccessReason; remainingMs: number } {
    this.expire()
    if (!this.userId) return { reason: 'account_signed_out', remainingMs: 0 }
    if (this.valid() && this.state.status === 'allowed')
      return {
        reason: 'allowed',
        remainingMs: Math.max(
          0,
          Math.floor(
            Math.min(
              this.deadline(this.lease!) - this.now(),
              this.monotonicDeadline - this.monotonic()
            )
          )
        )
      }
    return {
      reason:
        this.state.status === 'denied' || this.state.error === 'notProvisioned'
          ? 'license_required'
          : 'license_unavailable',
      remainingMs: 0
    }
  }
  /** Used only for one rate-limited return from a user-initiated management visit. */
  refreshAfterManagement(): Promise<LicenseState> {
    this.retryAt = 0
    return this.refresh()
  }
  refresh = (): Promise<LicenseState> => {
    if (!this.userId) return Promise.resolve(structuredClone(this.state))
    if (this.pending) return this.pending
    if (this.now() < this.retryAt) return Promise.resolve(structuredClone(this.state))
    const userId = this.userId
    const epoch = this.epoch
    const started = this.now()
    const startedMono = this.monotonic()
    const controller = new AbortController()
    this.controller = controller
    this.clearTimer()
    this.expire()
    if (!this.valid()) this.publish({ status: 'checking', error: null })
    const execute = async (): Promise<LicenseState> => {
      try {
        let result: VerifiedLicense
        try {
          result = await this.fetchLicense(this.auth.getAccessToken(userId), controller.signal)
        } catch (error) {
          if (!(error instanceof LicenseFailure) || !error.unauthorized) throw error
          const renewed = await this.auth.refreshProfile(true)
          if (!renewed.ok || epoch !== this.epoch) throw new LicenseFailure('network')
          result = await this.fetchLicense(this.auth.getAccessToken(userId), controller.signal)
        }
        if (epoch !== this.epoch) return structuredClone(this.state)
        result = parseVerifiedLicense(result)
        // Count request latency against the grant, never extend the server's 24h window.
        const elapsed = Math.max(this.now() - started, this.monotonic() - startedMono, 0)
        const entry = { userId, license: result, receivedAt: this.now() - elapsed }
        this.failures = 0
        this.retryAt = 0
        this.lease = null
        if (result.allowed && !this.acceptVerified(entry))
          throw new LicenseFailure('invalidResponse')
        if (!result.allowed) {
          this.publish({
            status: 'denied',
            reason: result.reason,
            expiresAt: result.expiresAt,
            verifiedAt: result.verifiedAt,
            cacheValidUntil: result.cacheValidUntil,
            error: null
          })
          // No tight polling of a known denial; an explicit retry remains available.
          this.retryAt = this.now() + 60_000
          this.schedule(60 * 60_000)
        }
      } catch (error) {
        if (epoch !== this.epoch) return structuredClone(this.state)
        const code = error instanceof LicenseFailure ? error.code : 'network'
        // A definitive missing grant is not a transient outage and must revoke an earlier lease.
        if (code === 'notProvisioned') {
          this.lease = null
          this.monotonicDeadline = 0
        }
        this.expire()
        this.publish({ status: this.valid() ? 'allowed' : 'unavailable', error: code })
        const delay = Math.min(60 * 60_000, 60_000 * 2 ** Math.min(this.failures++, 6))
        this.retryAt = this.now() + delay
        this.schedule(
          this.valid() ? Math.min(delay, this.deadline(this.lease!) - this.now()) : delay
        )
      }
      return structuredClone(this.state)
    }
    const pending = execute().finally(() => {
      if (epoch === this.epoch) {
        this.pending = null
        this.controller = null
      }
    })
    this.pending = pending
    return pending
  }
  dispose(): void {
    ++this.epoch
    this.unsubscribe()
    this.controller?.abort()
    this.clearTimer()
    this.listeners.clear()
  }
}
