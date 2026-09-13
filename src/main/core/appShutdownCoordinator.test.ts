import { describe, expect, it, vi } from 'vitest'
import { AppShutdownCoordinator } from '../lifecycle/AppShutdownCoordinator'

function deferred() {
  let resolve!: () => void
  let reject!: (error: unknown) => void
  const promise = new Promise<void>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

function harness() {
  const cleanup = deferred()
  const dependencies = {
    shutdownServices: vi.fn(() => cleanup.promise),
    quit: vi.fn(),
    relaunch: vi.fn(),
    onError: vi.fn()
  }
  const coordinator = new AppShutdownCoordinator(dependencies)
  return { cleanup, coordinator, dependencies }
}

describe('AppShutdownCoordinator', () => {
  it('drains services once before allowing a normal quit, including reentrant quit events', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    dependencies.quit.mockImplementation(() => {
      expect(coordinator.isQuittingAfterServiceShutdown).toBe(true)
      coordinator.requestQuit()
    })

    coordinator.requestQuit()
    coordinator.requestQuit()
    expect(coordinator.isServiceShutdownInProgress).toBe(true)
    expect(coordinator.isQuittingAfterServiceShutdown).toBe(false)
    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).not.toHaveBeenCalled()

    cleanup.resolve()
    await cleanup.promise

    expect(coordinator.isServiceShutdownInProgress).toBe(false)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
    expect(dependencies.relaunch).not.toHaveBeenCalled()
  })

  it('installs once only after cleanup and opens the quit gate before installer reentry', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const install = vi.fn(() => {
      expect(coordinator.isQuittingAfterServiceShutdown).toBe(true)
      coordinator.requestQuit()
      expect(coordinator.requestUpdateInstall(install)).toBe(false)
    })

    expect(coordinator.requestUpdateInstall(install)).toBe(true)
    expect(coordinator.requestUpdateInstall(install)).toBe(false)
    coordinator.requestQuit()
    expect(install).not.toHaveBeenCalled()

    cleanup.resolve()
    await cleanup.promise

    expect(install).toHaveBeenCalledTimes(1)
    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).not.toHaveBeenCalled()
    expect(dependencies.relaunch).not.toHaveBeenCalled()
  })

  it('rejects an update arriving after a normal quit has started or completed', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const install = vi.fn()

    coordinator.requestQuit()
    expect(coordinator.requestUpdateInstall(install)).toBe(false)
    cleanup.resolve()
    await cleanup.promise

    expect(coordinator.requestUpdateInstall(install)).toBe(false)
    expect(install).not.toHaveBeenCalled()
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
  })

  it('fences reentrant requests before invoking the synchronous part of cleanup', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    dependencies.shutdownServices.mockImplementation(() => {
      coordinator.requestQuit()
      expect(coordinator.requestUpdateInstall(vi.fn())).toBe(false)
      return cleanup.promise
    })

    coordinator.requestQuit()
    cleanup.resolve()
    await cleanup.promise

    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
  })

  it('preserves the normal exit fallback when service cleanup rejects', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const failure = new Error('cleanup failed')

    coordinator.requestQuit()
    cleanup.reject(failure)
    await cleanup.promise.catch(() => undefined)

    expect(coordinator.isQuittingAfterServiceShutdown).toBe(true)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(dependencies.onError).toHaveBeenCalledWith(
      'Failed to shut down application services',
      failure
    )
  })

  it('still opens the quit gate when the synchronous cleanup fence throws', async () => {
    const { coordinator, dependencies } = harness()
    dependencies.shutdownServices.mockImplementation(() => {
      throw new Error('service fence failed')
    })

    coordinator.requestQuit()
    await Promise.resolve()

    expect(coordinator.isQuittingAfterServiceShutdown).toBe(true)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
  })

  it('relaunches the old app once if installation throws, without retrying installation', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const failure = new Error('installer unavailable')
    const install = vi.fn(() => {
      throw failure
    })
    dependencies.quit.mockImplementation(() => coordinator.requestQuit())

    coordinator.requestUpdateInstall(install)
    cleanup.resolve()
    await cleanup.promise
    expect(coordinator.recoverFromUpdateInstallFailure(failure)).toBe(false)

    expect(dependencies.relaunch).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(install).toHaveBeenCalledTimes(1)
    expect(dependencies.shutdownServices).toHaveBeenCalledTimes(1)
  })

  it('handles rejected installation and a duplicate asynchronous installer error once', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const installation = deferred()
    const install = vi.fn(() => installation.promise)

    coordinator.requestUpdateInstall(install)
    cleanup.resolve()
    await cleanup.promise
    expect(dependencies.relaunch).not.toHaveBeenCalled()

    installation.reject(new Error('native updater failed'))
    await installation.promise.catch(() => undefined)
    expect(coordinator.recoverFromUpdateInstallFailure(new Error('duplicate event'))).toBe(false)

    expect(dependencies.relaunch).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(install).toHaveBeenCalledTimes(1)
  })

  it('accepts an error emitted inside installer dispatch after the quit gate has opened', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const install = vi.fn(() => {
      expect(coordinator.recoverFromUpdateInstallFailure(new Error('install event'))).toBe(true)
      throw new Error('duplicate thrown error')
    })

    coordinator.requestUpdateInstall(install)
    cleanup.resolve()
    await cleanup.promise

    expect(dependencies.relaunch).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
    expect(dependencies.onError).toHaveBeenCalledTimes(1)
  })

  it('recovers from a native installer error after its void dispatch returned, but not before it', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const install = vi.fn()
    coordinator.requestUpdateInstall(install)
    expect(coordinator.recoverFromUpdateInstallFailure(new Error('before dispatch'))).toBe(false)

    cleanup.resolve()
    await cleanup.promise
    expect(install).toHaveBeenCalledTimes(1)
    expect(coordinator.recoverFromUpdateInstallFailure(new Error('native error event'))).toBe(true)
    expect(coordinator.recoverFromUpdateInstallFailure(new Error('duplicate native event'))).toBe(
      false
    )

    expect(dependencies.relaunch).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
  })

  it('never relaunches for update errors before installation or during a normal quit', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    const failure = new Error('download failed')
    expect(coordinator.recoverFromUpdateInstallFailure(failure)).toBe(false)

    coordinator.requestQuit()
    expect(coordinator.recoverFromUpdateInstallFailure(failure)).toBe(false)
    cleanup.resolve()
    await cleanup.promise
    expect(coordinator.recoverFromUpdateInstallFailure(failure)).toBe(false)

    expect(dependencies.relaunch).not.toHaveBeenCalled()
    expect(dependencies.onError).not.toHaveBeenCalled()
  })

  it('still exits once if scheduling the old app relaunch fails', async () => {
    const { cleanup, coordinator, dependencies } = harness()
    dependencies.relaunch.mockImplementation(() => {
      throw new Error('relaunch unavailable')
    })

    coordinator.requestUpdateInstall(() => {
      throw new Error('install unavailable')
    })
    cleanup.resolve()
    await cleanup.promise
    coordinator.recoverFromUpdateInstallFailure(new Error('duplicate event'))

    expect(coordinator.isQuittingAfterServiceShutdown).toBe(true)
    expect(dependencies.relaunch).toHaveBeenCalledTimes(1)
    expect(dependencies.quit).toHaveBeenCalledTimes(1)
  })
})
