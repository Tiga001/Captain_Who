import { describe, expect, it, vi } from 'vitest'
import { TrailingRefresh } from './trailingRefresh'

function deferred() {
  let resolve!: () => void
  let reject!: (error: unknown) => void
  const promise = new Promise<void>((accept, fail) => {
    resolve = accept
    reject = fail
  })
  return { promise, resolve, reject }
}

describe('TrailingRefresh', () => {
  it('retains one latest trailing read and makes its callers await that read', async () => {
    const queue = new TrailingRefresh()
    const first = deferred()
    const last = deferred()
    const reads = vi.fn(() => last.promise)
    const active = queue.request(() => first.promise)
    const pending = Array.from({ length: 100 }, () => queue.request(reads))
    expect(new Set(pending).size).toBe(1)
    await Promise.resolve()
    expect(reads).not.toHaveBeenCalled()
    first.resolve()
    await active
    await Promise.resolve()
    expect(reads).toHaveBeenCalledTimes(1)
    let settled = false
    void pending[0]!.then(() => {
      settled = true
    })
    await Promise.resolve()
    expect(settled).toBe(false)
    last.resolve()
    await Promise.all(pending)
    expect(settled).toBe(true)
  })

  it('preserves failure for callers and still runs the next invalidation', async () => {
    const queue = new TrailingRefresh()
    const first = deferred()
    const next = vi.fn(async () => {})
    const active = queue.request(() => first.promise)
    const failure = expect(active).rejects.toThrow('read failed')
    const pending = queue.request(next)
    first.reject(new Error('read failed'))
    await failure
    await pending
    expect(next).toHaveBeenCalledTimes(1)
  })
})
