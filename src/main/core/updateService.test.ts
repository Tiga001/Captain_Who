import { describe, expect, it, vi } from 'vitest'
import { UpdateService, type DesktopUpdateDriver } from '../updates/UpdateService'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}
const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}
function setup() {
  const check = deferred<{ version: string } | null>()
  const download = deferred<void>()
  let progress!: (percent: number) => void
  let installError!: (error: unknown) => void
  const unsubscribe = vi.fn()
  const driver = {
    check: vi.fn(() => check.promise),
    download: vi.fn((notify) => {
      progress = notify
      return download.promise
    }),
    install: vi.fn(),
    cancel: vi.fn(),
    dispose: vi.fn(),
    onInstallError: vi.fn((notify) => {
      installError = notify
      return unsubscribe
    })
  } satisfies DesktopUpdateDriver
  const lifecycle = {
    requestInstall: vi.fn((install: () => void) => typeof install === 'function'),
    onInstallFailure: vi.fn(),
    isShuttingDown: vi.fn(() => false)
  }
  const service = new UpdateService(driver, lifecycle)
  return {
    service,
    driver,
    lifecycle,
    check,
    download,
    unsubscribe,
    progress: (n: number) => progress(n),
    installError: (e: unknown) => installError(e)
  }
}
async function available() {
  const test = setup()
  test.service.startOnce()
  test.check.resolve({ version: '1.0.1' })
  await flush()
  return test
}

describe('desktop update state machine', () => {
  it('stays disabled with no configured driver', () => {
    const service = new UpdateService(null, setup().lifecycle)
    service.startOnce()
    expect(service.download().status).toBe('disabled')
  })
  it('checks once and never downloads without an explicit request', async () => {
    const t = await available()
    t.service.startOnce()
    expect(t.driver.check).toHaveBeenCalledTimes(1)
    expect(t.driver.download).not.toHaveBeenCalled()
    expect(t.service.getState()).toMatchObject({ status: 'available', version: '1.0.1' })
  })
  it('keeps idle and check-failed states non-downloadable without exposing raw errors', async () => {
    for (const fail of [false, true]) {
      const t = setup()
      t.service.startOnce()
      if (fail) t.check.reject(new Error('private.example/token=secret'))
      else t.check.resolve(null)
      await flush()
      expect(t.service.download()).toMatchObject({
        status: fail ? 'error' : 'idle',
        version: null,
        error: fail ? 'checkFailed' : null
      })
      expect(t.driver.download).not.toHaveBeenCalled()
      expect(JSON.stringify(t.service.getState())).not.toContain('secret')
    }
  })
  it('returns immediately, ignores duplicate clicks and clamps monotonic progress', async () => {
    const t = await available()
    expect(t.service.download().status).toBe('downloading')
    t.service.download()
    expect(t.driver.download).toHaveBeenCalledTimes(1)
    for (const n of [-1, 22.9, 20, NaN, Infinity]) t.progress(n)
    expect(t.service.getState().percent).toBe(22)
    t.progress(102)
    expect(t.service.getState()).toMatchObject({ status: 'preparing', percent: 100 })
    const preparing = t.service.getState()
    for (const n of [50, 100, 101]) t.progress(n)
    t.service.download()
    expect(t.service.getState()).toEqual(preparing)
    expect(t.driver.download).toHaveBeenCalledTimes(1)
    expect(t.lifecycle.requestInstall).not.toHaveBeenCalled()
  })
  it('only requests safe shutdown after native preparation resolves', async () => {
    const t = await available()
    t.service.download()
    t.progress(100)
    expect(t.service.getState()).toMatchObject({ status: 'preparing', percent: 100 })
    expect(t.lifecycle.requestInstall).not.toHaveBeenCalled()
    t.download.resolve()
    await flush()
    expect(t.service.getState()).toMatchObject({ status: 'installing', percent: 100 })
    expect(t.lifecycle.requestInstall).toHaveBeenCalledTimes(1)
    expect(t.driver.install).not.toHaveBeenCalled()
    t.lifecycle.requestInstall.mock.calls[0][0]()
    expect(t.driver.install).toHaveBeenCalledTimes(1)
  })
  it.each(['downloading', 'preparing'] as const)(
    'restores a retryable button on %s failure without shutting down',
    async (phase) => {
      const t = await available()
      t.service.download()
      if (phase === 'preparing') t.progress(100)
      t.download.reject(new Error('secret signed URL'))
      await flush()
      expect(t.service.getState()).toMatchObject({
        status: 'error',
        version: '1.0.1',
        error: 'downloadFailed'
      })
      expect(t.lifecycle.requestInstall).not.toHaveBeenCalled()
      const retry = deferred<void>()
      t.driver.download.mockImplementationOnce(() => retry.promise)
      expect(t.service.download()).toMatchObject({ status: 'downloading', percent: 0, error: null })
      expect(t.driver.download).toHaveBeenCalledTimes(2)
      t.service.dispose()
      retry.resolve()
    }
  )
  it('ignores late checks after shutdown', async () => {
    const t = setup()
    t.service.startOnce()
    t.service.beginShutdown()
    const revision = t.service.getState().revision
    t.check.resolve({ version: '1.0.1' })
    await flush()
    expect(t.service.getState().revision).toBe(revision)
    expect(t.driver.cancel).toHaveBeenCalledTimes(1)
  })
  it.each(['downloading', 'preparing'] as const)(
    'cancels ordinary quit during %s and ignores late progress/completion',
    async (phase) => {
      const t = await available()
      t.service.download()
      if (phase === 'preparing') t.progress(100)
      t.service.beginShutdown()
      const state = t.service.getState()
      t.progress(90)
      t.download.resolve()
      await flush()
      expect(t.service.getState()).toEqual(state)
      expect(t.lifecycle.requestInstall).not.toHaveBeenCalled()
    }
  )
  it('does not cancel a prepared installation when the coordinator starts cleanup', async () => {
    const t = await available()
    t.lifecycle.requestInstall.mockImplementation(() => {
      t.service.beginShutdown()
      return true
    })
    t.service.download()
    t.download.resolve()
    await flush()
    expect(t.driver.cancel).not.toHaveBeenCalled()
    t.installError('native failure')
    expect(t.lifecycle.onInstallFailure).toHaveBeenCalledWith('native failure')
  })
  it('respects a concurrent normal-quit fence and rejected install handoff', async () => {
    const t = await available()
    t.lifecycle.isShuttingDown.mockReturnValue(true)
    t.service.download()
    expect(t.driver.download).not.toHaveBeenCalled()
    t.lifecycle.isShuttingDown.mockReturnValue(false)
    t.lifecycle.requestInstall.mockReturnValue(false)
    t.service.download()
    t.download.resolve()
    await flush()
    expect(t.service.getState()).toMatchObject({ status: 'error', error: 'installFailed' })
    expect(t.driver.install).not.toHaveBeenCalled()
  })
  it('publishes detached revisioned projections and removes subscriptions', async () => {
    const t = setup()
    const listener = vi.fn()
    const unsubscribe = t.service.subscribe(listener)
    t.service.startOnce()
    const projection = t.service.getState()
    projection.status = 'installing'
    expect(t.service.getState().status).toBe('checking')
    t.check.resolve(null)
    await flush()
    expect(listener.mock.calls.map(([s]) => s.revision)).toEqual([1, 2])
    unsubscribe()
    t.service.dispose()
    expect(t.unsubscribe).toHaveBeenCalledOnce()
    expect(t.driver.dispose).toHaveBeenCalledOnce()
  })
  it('uses the install error projection if the handoff throws before cleanup starts', async () => {
    const t = await available()
    t.lifecycle.requestInstall.mockImplementation(() => {
      throw new Error('handoff unavailable')
    })
    t.service.download()
    t.download.resolve()
    await flush()
    expect(t.service.getState()).toMatchObject({ status: 'error', error: 'installFailed' })
    expect(t.driver.install).not.toHaveBeenCalled()
  })
})
