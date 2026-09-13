import { EXECUTION_ACCESS_MAX_TTL_MS, type ExecutionAccessSnapshot } from '@mycopilot/protocol'
import type { AuthService } from './AuthService'
import type { LicenseService } from './LicenseService'
import type { CoreServer } from '../core/coreServer'

type Core = Pick<CoreServer, 'onStarted' | 'isRunning' | 'setExecutionAccess'>

/** Refreshes a local IPC lease, not a cloud license. Never starts or stops the Core process. */
export class ExecutionAccessBridge {
  private revision = 0
  private identityEpoch = 0
  private identity: string | null = null
  private disposed = false
  private readonly cleanup: Array<() => void>
  private readonly timer: ReturnType<typeof setInterval>

  constructor(
    private readonly auth: Pick<AuthService, 'getState' | 'subscribe'>,
    private readonly license: Pick<LicenseService, 'getExecutionLease' | 'subscribe'>,
    private readonly core: Core,
    private readonly now: () => number = Date.now
  ) {
    const send = (): void => {
      void this.sync().catch(() => {
        /* Core fails closed if delivery fails. */
      })
    }
    this.cleanup = [auth.subscribe(send), license.subscribe(send), core.onStarted(send)]
    this.timer = setInterval(send, 30_000)
    this.timer.unref?.()
    send()
  }

  async sync(): Promise<void> {
    if (this.disposed || !this.core.isRunning()) return
    const auth = this.auth.getState()
    const nextIdentity = auth.status === 'signedIn' ? (auth.profile?.userId ?? null) : null
    if (nextIdentity !== this.identity) {
      this.identity = nextIdentity
      ++this.identityEpoch
    }
    const lease = this.license.getExecutionLease()
    const issuedAt = this.now()
    const allowedMs =
      nextIdentity && lease.reason === 'allowed'
        ? Math.min(EXECUTION_ACCESS_MAX_TTL_MS, lease.remainingMs)
        : 0
    const snapshot: ExecutionAccessSnapshot = {
      revision: ++this.revision,
      identityEpoch: this.identityEpoch,
      reason: !nextIdentity
        ? 'account_signed_out'
        : allowedMs > 0
          ? 'allowed'
          : lease.reason === 'allowed' || lease.reason === 'account_signed_out'
            ? 'license_unavailable'
            : lease.reason,
      issuedAt,
      validUntil: issuedAt + allowedMs
    }
    const receipt = await this.core.setExecutionAccess(snapshot)
    if (receipt.revision < snapshot.revision) throw new Error('Execution access was not applied')
  }

  dispose(): void {
    this.disposed = true
    clearInterval(this.timer)
    for (const unsubscribe of this.cleanup) unsubscribe()
  }
}
