interface AppShutdownDependencies {
  shutdownServices(): Promise<void>
  quit(): void
  relaunch(): void
  onError(message: string, error: unknown): void
}

type ShutdownIntent = { kind: 'quit' } | { kind: 'update'; install: () => void | Promise<void> }

/**
 * Owns one shutdown transaction. Electron may emit before-quit again when the final quit or
 * installer runs; those events must pass through without starting another cleanup transaction.
 */
export class AppShutdownCoordinator {
  private intent: ShutdownIntent | null = null
  private servicesStopped = false
  private installDispatched = false
  private recoveryStarted = false

  constructor(private readonly dependencies: AppShutdownDependencies) {}

  get isServiceShutdownInProgress(): boolean {
    return this.intent !== null && !this.servicesStopped
  }

  get isQuittingAfterServiceShutdown(): boolean {
    return this.servicesStopped
  }

  requestQuit(): void {
    if (this.intent) return
    this.beginShutdown({ kind: 'quit' })
  }

  /** A normal quit already in flight cannot be converted into an update installation. */
  requestUpdateInstall(install: () => void | Promise<void>): boolean {
    if (this.intent) return false
    this.beginShutdown({ kind: 'update', install })
    return true
  }

  /** Also accepts installer errors emitted asynchronously after its dispatch method returned. */
  recoverFromUpdateInstallFailure(error: unknown): boolean {
    if (
      this.intent?.kind !== 'update' ||
      !this.servicesStopped ||
      !this.installDispatched ||
      this.recoveryStarted
    ) {
      return false
    }

    // Core admission and Renderer persistence queues have already been sealed. Recovery must
    // start a fresh process; showing the old window would expose an unusable application.
    this.recoveryStarted = true
    this.dependencies.onError('Failed to install the downloaded application update', error)
    try {
      this.dependencies.relaunch()
    } catch (relaunchError) {
      this.dependencies.onError(
        'Failed to relaunch after update installation failed',
        relaunchError
      )
    } finally {
      this.quit()
    }
    return true
  }

  private beginShutdown(intent: ShutdownIntent): void {
    // Set the intent before invoking cleanup: notification fencing and hiding the live Renderer
    // happen synchronously, and a reentrant request must already see the transaction in progress.
    this.intent = intent
    let cleanup: Promise<void>
    try {
      cleanup = this.dependencies.shutdownServices()
    } catch (error) {
      cleanup = Promise.reject(error)
    }
    void cleanup.then(
      () => this.finishShutdown(intent),
      (error) => {
        // Preserve normal exit's existing fail-open behavior after bounded cleanup has failed.
        this.dependencies.onError('Failed to shut down application services', error)
        this.finishShutdown(intent)
      }
    )
  }

  private finishShutdown(intent: ShutdownIntent): void {
    // This flag must precede quitAndInstall: macOS can close windows and emit before-quit from
    // inside the installer, and both paths need to bypass close-to-hide and further cleanup.
    this.servicesStopped = true
    if (intent.kind === 'quit') {
      this.quit()
      return
    }

    this.installDispatched = true
    try {
      void Promise.resolve(intent.install()).catch((error) => {
        this.recoverFromUpdateInstallFailure(error)
      })
    } catch (error) {
      this.recoverFromUpdateInstallFailure(error)
    }
  }

  private quit(): void {
    try {
      this.dependencies.quit()
    } catch (error) {
      this.dependencies.onError('Failed to quit after application service shutdown', error)
    }
  }
}
