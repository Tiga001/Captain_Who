import type { AccountProfile, AuthActionResult, AuthState } from '@mycopilot/host-api'
import { AuthFailure, type AuthDriver, type CloudSession } from './CloudBaseAuthDriver'
import type { SessionStorage } from './SessionStore'

export class AuthService {
  private state: AuthState = {
    revision: 0,
    status: 'checking',
    profile: null,
    error: null,
    remembered: false
  }
  private session: CloudSession | null = null
  private epoch = 0
  private queue: Promise<unknown> = Promise.resolve()
  private listeners = new Set<(state: AuthState) => void>()
  private lastCodeSentAt = 0
  private lastValidationAt = 0
  private refreshing: Promise<AuthActionResult> | null = null

  constructor(
    private readonly driver: AuthDriver,
    private readonly store: SessionStorage,
    private readonly fetchProfile: (session: CloudSession) => Promise<AccountProfile>
  ) {}

  getState = (): AuthState => structuredClone(this.state)

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
            let storageError = false
            try {
              this.store.clear()
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

  private async accept(session: CloudSession, epoch: number): Promise<void> {
    if (epoch !== this.epoch) return
    this.session = session
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
      session = await this.driver.refresh(session)
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
      this.publish({ status: 'checking', profile: null, error: null })
      if (this.session) return this.accept(await this.driver.refresh(this.session), epoch)
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
      await this.accept(await this.driver.restore(tokens), epoch)
    })

  login(input: { email: string; password: string }): Promise<AuthActionResult> {
    return this.run(async (epoch) => {
      const email = normalizeEmail(input?.email)
      if (typeof input?.password !== 'string' || !input.password || input.password.length > 1024)
        throw new AuthFailure('credentials')
      this.prepareNewIdentity()
      await this.accept(await this.driver.login(email, input.password), epoch)
    })
  }

  sendEmailCode(email: string): Promise<AuthActionResult> {
    return this.run(async () => {
      email = normalizeEmail(email)
      if (Date.now() - this.lastCodeSentAt < 60_000) throw new AuthFailure('rateLimit')
      await this.driver.sendCode(email)
      // Start the server-side retry window only after CloudBase accepted the
      // request. A failed request must be immediately retryable.
      this.lastCodeSentAt = Date.now()
    })
  }

  verifyEmailCode(input: { email: string; code: string }): Promise<AuthActionResult> {
    return this.run(async (epoch) => {
      const email = normalizeEmail(input?.email)
      if (typeof input?.code !== 'string' || !/^\d{4,8}$/.test(input.code.trim()))
        throw new AuthFailure('code')
      this.prepareNewIdentity()
      await this.accept(await this.driver.verifyCode(email, input.code.trim()), epoch)
    })
  }

  logout = async (): Promise<AuthActionResult> => {
    // Synchronous invalidation prevents an in-flight login/refresh from signing back in later.
    ++this.epoch
    this.session = null
    let failed = false
    try {
      this.store.clear()
    } catch {
      failed = true
    }
    this.publish({
      status: 'signedOut',
      profile: null,
      error: failed ? 'storage' : null,
      remembered: false
    })
    const revoke = async (): Promise<void> => {
      try {
        await this.driver.logout()
      } catch {
        /* Local logout remains effective offline. */
      }
    }
    this.queue = this.queue.then(revoke, revoke)
    return failed ? { ok: false, error: 'storage' } : { ok: true }
  }

  private prepareNewIdentity(): void {
    // Never fall back to an older account on next launch if saving the new session fails.
    try {
      this.store.clear()
    } catch {
      throw new AuthFailure('storage')
    }
    this.session = null
    this.publish({ status: 'signedOut', profile: null, error: null, remembered: false })
  }

  refreshProfile = (force = true): Promise<AuthActionResult> => {
    if (this.refreshing) return this.refreshing
    if (
      this.state.status !== 'signedIn' ||
      (!force && Date.now() - this.lastValidationAt < 5 * 60_000)
    )
      return Promise.resolve({ ok: true })
    this.refreshing = this.run(async (epoch) => {
      if (!this.session) return
      await this.accept(await this.driver.refresh(this.session), epoch)
    }).finally(() => {
      this.refreshing = null
    })
    return this.refreshing
  }
}

function normalizeEmail(value: unknown): string {
  if (typeof value !== 'string') throw new AuthFailure('credentials')
  const email = value.trim().toLowerCase()
  if (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email))
    throw new AuthFailure('credentials')
  return email
}
