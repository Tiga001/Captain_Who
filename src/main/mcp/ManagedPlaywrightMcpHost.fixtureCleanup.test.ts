import { describe, expect, it, vi } from 'vitest'
import type { ManagedPlaywrightMcpHost } from './ManagedPlaywrightMcpHost'
import { createManagedPlaywrightHostTestFixture } from './ManagedPlaywrightMcpHost.test-fixtures'

describe('ManagedPlaywrightMcpHost fixture cleanup', () => {
  it('keeps host tracking and authorization state isolated between fixture instances', async () => {
    const left = createManagedPlaywrightHostTestFixture()
    const right = createManagedPlaywrightHostTestFixture()
    const leftHost = closingHost(async () => undefined)
    const rightHost = closingHost(async () => undefined)
    left.trackedHosts.add(leftHost)
    right.trackedHosts.add(rightHost)

    expect(left.trackedHosts).not.toBe(right.trackedHosts)
    expect(left.RISK_CONTEXT).not.toBe(right.RISK_CONTEXT)
    await left.closeTrackedHosts()

    expect(leftHost.close).toHaveBeenCalledOnce()
    expect(rightHost.close).not.toHaveBeenCalled()
    expect(left.trackedHosts.size).toBe(0)
    expect(right.trackedHosts.has(rightHost)).toBe(true)

    await right.closeTrackedHosts()
    expect(rightHost.close).toHaveBeenCalledOnce()
    expect(right.trackedHosts.size).toBe(0)
  })

  it('closes every host and reports all failures while leaving repeated cleanup safe', async () => {
    const fixture = createManagedPlaywrightHostTestFixture()
    const synchronousFailure = new Error('synchronous close failure')
    const asynchronousFailure = new Error('asynchronous close failure')
    let finishSlowClose!: () => void
    const slowClose = new Promise<void>((resolve) => {
      finishSlowClose = resolve
    })
    const synchronousHost = closingHost(() => {
      throw synchronousFailure
    })
    const asynchronousHost = closingHost(async () => {
      throw asynchronousFailure
    })
    const slowHost = closingHost(async () => slowClose)
    for (const host of [synchronousHost, asynchronousHost, slowHost]) {
      fixture.trackedHosts.add(host)
    }

    let settled = false
    const outcome = fixture.closeTrackedHosts().then(
      () => {
        settled = true
        return undefined
      },
      (error: unknown) => {
        settled = true
        return error
      }
    )
    try {
      expect(fixture.trackedHosts.size).toBe(0)
      expect(synchronousHost.close).toHaveBeenCalledOnce()
      expect(asynchronousHost.close).toHaveBeenCalledOnce()
      expect(slowHost.close).toHaveBeenCalledOnce()
      await Promise.resolve()
      expect(settled).toBe(false)
    } finally {
      finishSlowClose()
    }

    const failure = await outcome
    expect(failure).toBeInstanceOf(AggregateError)
    expect((failure as AggregateError).errors).toEqual([synchronousFailure, asynchronousFailure])
    await expect(fixture.closeTrackedHosts()).resolves.toBeUndefined()
    expect(fixture.trackedHosts.size).toBe(0)
    expect(synchronousHost.close).toHaveBeenCalledOnce()
    expect(asynchronousHost.close).toHaveBeenCalledOnce()
    expect(slowHost.close).toHaveBeenCalledOnce()
  })
})

// Cleanup-only doubles allocate no real connections, browser contexts, or files.
function closingHost(close: () => Promise<void>): ManagedPlaywrightMcpHost {
  return { close: vi.fn(close) } as unknown as ManagedPlaywrightMcpHost
}
