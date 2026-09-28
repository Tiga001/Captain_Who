import type { AccountProfile, AuthActionResult, AuthState } from '@mycopilot/host-api'
import { AuthFailure, type AuthDriver, type CloudSession } from './CloudBaseAuthDriver'
import type { SessionStorage } from './SessionStore'
import {
  acceptsLocalPassword,
  LOCAL_ACCOUNT_USERNAME,
  localAccountProfile,
  type LocalAccountSession,
  type LocalAccountStorage
} from './LocalAccountStore'

export class AuthService {
  private state: AuthState = {
    revision: 0,
    status: 'checking',
    profile: null,
    error: null,
    remembered: false
  }
  private session: CloudSession | null = null
  private localSession: LocalAccountSession | null = null
  private epoch = 0
  private queue: Promise<unknown> = Promise.resolve()
  private cloudQueue: Promise<unknown> = Promise.resolve()
  private listeners = new Set<(state: AuthState) => void>()
  private lastCodeSentAt = 0
  private lastValidationAt = 0
  private refreshing: Promise<AuthActionResult> | null = null

  constructor(
    private readonly driver: AuthDriver,
    private readonly store: SessionStorage,
    private readonly fetchProfile: (session: CloudSession) => Promise<AccountProfile>,
    private readonly localStore?: LocalAccountStorage
  ) {}

  getState = (): AuthState => structuredClone(this.state)

  /** Main-only identity check. Profile metadata is never an authorization source. */
  isLocalAccount = (): boolean => this.state.status === 'signedIn' && this.localSession !== null

  subscribe(listener: (state: AuthState) => void): () => void {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  private publish(patch: Partial<AuthState>): void {
    this.state = { ...this.state, ...patch, revision: this.state.revision + 1 }
    for (const listener of this.listeners) listener(this.getState())
  }

  assertCanStartTurn(): void {
    if (this.state.status !== 'signedIn') throw new Error('ACCOUNT_LOGIN_REQUIRED')
  }

  /** Main-only access for authenticated account services. Never expose this through IPC. */
  getAccessToken(userId: string): string {
    this.assertCanStartTurn()
    if (!this.session || this.state.profile?.userId !== userId)
      throw new Error('ACCOUNT_LOGIN_REQUIRED')
    return this.session.access_token
  }

  private run(operation: (epoch: number) => Promise<void>): Promise<AuthActionResult> {
    const epoch = this.epoch
    const execute = async (): Promise<AuthActionResult> => {
      if (epoch !== this.epoch) return { ok: false, error: 'expired' }
      try {
        if (this.state.error) this.publish({ error: null })
        await operation(epoch)
        return epoch === this.epoch ? { ok: true } : { ok: false, error: 'expired' }
      } catch (error) {
        const code = error instanceof AuthFailure ? error.code : 'network'
        if (epoch === this.epoch) {
          if (code === 'expired' || code === 'inactive') {
            this.session = null
            this.localSession = null
            let storageError = false
            try {
              this.clearPersistedSessions()
            } catch {
              storageError = true
            }
            this.publish({
              status: 'signedOut',
              profile: null,
              remembered: false,
              error: storageError ? 'storage' : code
            })
          } else if (code === 'profile' || this.state.status !== 'signedIn') {
            this.publish({
              status: this.session || code === 'storage' ? 'error' : 'signedOut',
              profile: null,
              error: code
            })
          }
        }
        return { ok: false, error: code }
      }
    }
    const result = this.queue.then(execute, execute)
    this.queue = result
    return result
  }

  private runCloud<T>(operation: () => Promise<T>, epoch?: number): Promise<T> {
    // The SDK owns mutable session state, including getUser after each token operation.
    // Local sign-in can detach the service queue, but must never let two SDK calls overlap.
    const execute = (): Promise<T> => {
      if (epoch !== undefined && epoch !== this.epoch) throw new AuthFailure('expired')
      return operation()
    }
    const result = this.cloudQueue.then(execute, execute)
    this.cloudQueue = result
    return result
  }

  private async accept(session: CloudSession, epoch: number): Promise<void> {
    if (epoch !== this.epoch) return
    this.session = session
    this.localSession = null
    let remembered = false
    // Persist rotated refresh tokens before /me: a transient profile outage must not lose them.
    try {
      remembered = this.store.write(session)
    } catch {
      /* Continue with a memory-only session. */
    }
    let profile: AccountProfile
    try {
      profile = await this.fetchProfile(session)
    } catch (error) {
      if (!(error instanceof AuthFailure) || error.code !== 'expired' || epoch !== this.epoch)
        throw error
      session = await this.runCloud(() => this.driver.refresh(session), epoch)
      if (epoch !== this.epoch) return
      this.session = session
      try {
        remembered = this.store.write(session)
      } catch {
        remembered = false
      }
      profile = await this.fetchProfile(session)
    }
    if (epoch !== this.epoch) return
    this.lastValidationAt = Date.now()
    this.publish({ status: 'signedIn', profile, error: null, remembered })
  }

  restoreSession = (): Promise<AuthActionResult> =>
    this.run(async (epoch) => {
      if (this.localSession) {
        this.acceptLocal(this.localSession)
        return
      }
      this.publish({ status: 'checking', profile: null, error: null })
      let local
      try {
        local = this.localStore?.read()
      } catch {
        throw new AuthFailure('storage')
      }
      if (local) {
        this.acceptLocal(local)
        return
      }
      const session = this.session
      if (session)
        return this.accept(await this.runCloud(() => this.driver.refresh(session), epoch), epoch)
      let tokens
      try {
        tokens = this.store.read()
      } catch {
        throw new AuthFailure('storage')
      }
      if (!tokens) {
        this.publish({ status: 'signedOut', profile: null, error: null, remembered: false })
        return
      }
      await this.accept(await this.runCloud(() => this.driver.restore(tokens), epoch), epoch)
    })

  login(input: { email: string; password: string }): Promise<AuthActionResult> {
    if (
      typeof input?.email === 'string' &&
      input.email.trim().toLowerCase() === LOCAL_ACCOUNT_USERNAME
    ) {
      if (!acceptsLocalPassword(input.password))
        return Promise.resolve({ ok: false, error: 'credentials' })
      // Offline sign-in must not wait for an unreachable cloud restore/refresh. Fence late
      // cloud results before starting an independent local operation; they cannot restore it.
      ++this.epoch
      this.refreshing = null
      this.queue = Promise.resolve()
      return this.run(async () => {
        this.prepareNewIdentity()
        if (!this.localStore) throw new AuthFailure('storage')
        let session: LocalAccountSession
        try {
          session = this.localStore.activate()
        } catch {
          throw new AuthFailure('storage')
        }
        this.acceptLocal(session)
      })
    }
    return this.run(async (epoch) => {
      const email = normalizeEmail(input?.email)
      if (typeof input?.password !== 'string' || !input.password || input.password.length > 1024)
        throw new AuthFailure('credentials')
      this.prepareNewIdentity()
      await this.accept(
        await this.runCloud(() => this.driver.login(email, input.password), epoch),
        epoch
      )
    })
  }

  sendEmailCode(email: string): Promise<AuthActionResult> {
    return this.run(async (epoch) => {
      email = normalizeEmail(email)
      if (Date.now() - this.lastCodeSentAt < 60_000) throw new AuthFailure('rateLimit')
      await this.runCloud(() => this.driver.sendCode(email), epoch)
      // Start the server-side retry window only after CloudBase accepted the
      // request. A failed request must be immediately retryable.
      if (epoch === this.epoch) this.lastCodeSentAt = Date.now()
    })
  }

  verifyEmailCode(input: { email: string; code: string }): Promise<AuthActionResult> {
    return this.run(async (epoch) => {
      const email = normalizeEmail(input?.email)
      if (typeof input?.code !== 'string' || !/^\d{4,8}$/.test(input.code.trim()))
        throw new AuthFailure('code')
      this.prepareNewIdentity()
      await this.accept(
        await this.runCloud(() => this.driver.verifyCode(email, input.code.trim()), epoch),
        epoch
      )
    })
  }

  logout = async (): Promise<AuthActionResult> => {
    // Synchronous invalidation prevents an in-flight login/refresh from signing back in later.
    ++this.epoch
    const wasLocal = this.localSession !== null
    this.session = null
    this.localSession = null
    let failed = false
    try {
      this.clearPersistedSessions()
    } catch {
      failed = true
    }
    this.publish({
      status: 'signedOut',
      profile: null,
      error: failed ? 'storage' : null,
      remembered: false
    })
    if (!wasLocal) {
      // Queue revocation now, not after the detachable service queue: a later cloud
      // login must always run after this signOut, even if local sign-in intervenes.
      const revoked = this.runCloud(async () => {
        try {
          await this.driver.logout()
        } catch {
          /* Local logout remains effective offline. */
        }
      })
      this.queue = this.queue.then(
        () => revoked,
        () => revoked
      )
    }
    return failed ? { ok: false, error: 'storage' } : { ok: true }
  }

  private prepareNewIdentity(): void {
    // Never fall back to an older account on next launch if saving the new session fails.
    try {
      this.clearPersistedSessions()
    } catch {
      throw new AuthFailure('storage')
    }
    this.session = null
    this.localSession = null
    this.publish({ status: 'signedOut', profile: null, error: null, remembered: false })
  }

  refreshProfile = (force = true): Promise<AuthActionResult> => {
    if (this.isLocalAccount()) return Promise.resolve({ ok: true })
    if (this.refreshing) return this.refreshing
    if (
      this.state.status !== 'signedIn' ||
      (!force && Date.now() - this.lastValidationAt < 5 * 60_000)
    )
      return Promise.resolve({ ok: true })
    this.refreshing = this.run(async (epoch) => {
      const session = this.session
      if (!session) return
      await this.accept(await this.runCloud(() => this.driver.refresh(session), epoch), epoch)
    }).finally(() => {
      this.refreshing = null
    })
    return this.refreshing
  }

  private acceptLocal(session: LocalAccountSession): void {
    this.session = null
    this.localSession = session
    this.publish({
      status: 'signedIn',
      profile: localAccountProfile(session),
      error: null,
      remembered: true
    })
  }

  private clearPersistedSessions(): void {
    // Attempt both removals even if one fails, so switching cannot revive an older identity.
    let failed = false
    for (const store of [this.store, this.localStore]) {
      try {
        store?.clear()
      } catch {
        failed = true
      }
    }
    if (failed) throw new AuthFailure('storage')
  }
}

function normalizeEmail(value: unknown): string {
  if (typeof value !== 'string') throw new AuthFailure('credentials')
  const email = value.trim().toLowerCase()
  if (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))
    throw new AuthFailure('credentials')
  return email
}
