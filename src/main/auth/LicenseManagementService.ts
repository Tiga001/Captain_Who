import type { AuthService } from './AuthService'
import type { LicenseService } from './LicenseService'
import { ACCOUNT_PAGES } from './accountConfig'

/** Explicit visits only. Background checks never open a browser or change window focus. */
export class LicenseManagementService {
  private pending: { userId: string; openedAt: number; blurred: boolean } | null = null
  private opening: Promise<void> | null = null
  private lastOpened = -Infinity
  private lastRecheck = -Infinity

  constructor(
    private readonly auth: Pick<AuthService, 'getState'>,
    private readonly license: Pick<LicenseService, 'refreshAfterManagement'>,
    private readonly openExternal: (url: string) => Promise<void>,
    private readonly now: () => number = Date.now
  ) {}

  open = (): Promise<void> => {
    const state = this.auth.getState()
    const userId = state.status === 'signedIn' ? state.profile?.userId : null
    if (!userId) return Promise.reject(new Error('ACCOUNT_LOGIN_REQUIRED'))
    if (this.opening) return this.opening
    if (this.now() - this.lastOpened < 10_000) return Promise.resolve()
    const visit = { userId, openedAt: this.now(), blurred: false }
    this.pending = visit
    this.lastOpened = visit.openedAt
    this.opening = Promise.resolve()
      .then(() => this.openExternal(ACCOUNT_PAGES.profile))
      .catch(() => {
        if (this.pending === visit) this.pending = null
        this.lastOpened = -Infinity
        throw new Error('Unable to open license management')
      })
      .finally(() => {
        this.opening = null
      })
    return this.opening
  }

  onBlur = (): void => {
    if (this.pending) this.pending.blurred = true
  }
  onFocus = (): void => {
    const visit = this.pending
    if (!visit?.blurred) return
    this.pending = null
    const state = this.auth.getState()
    const age = this.now() - visit.openedAt
    if (
      age < 0 ||
      age > 10 * 60_000 ||
      state.status !== 'signedIn' ||
      state.profile?.userId !== visit.userId ||
      this.now() - this.lastRecheck < 30_000
    )
      return
    this.lastRecheck = this.now()
    // No automatic replay of a blocked message or automation after a grant is restored.
    void this.license.refreshAfterManagement().catch(() => {
      /* Safe status remains in LicenseService. */
    })
  }

  dispose(): void {
    this.pending = null
  }
}
