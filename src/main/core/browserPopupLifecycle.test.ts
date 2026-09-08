import { EventEmitter } from 'node:events'
import type { WebContents } from 'electron'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { observeNativePopupLoad } from '../browser/BrowserPopupLifecycle'

afterEach(() => vi.useRealTimers())

describe('native popup initial navigation lifetime', () => {
  it('waits through blank initialization and does not count time spent awaiting approval', async () => {
    vi.useFakeTimers()
    const guest = new EventEmitter()
    const lifecycle = observeNativePopupLoad(guest as unknown as WebContents)
    const settled = vi.fn()
    void lifecycle.settled.then(settled)
    guest.emit('did-finish-load')
    await vi.advanceTimersByTimeAsync(20_000)
    expect(settled).not.toHaveBeenCalled()
    lifecycle.start()
    guest.emit('did-start-navigation', {}, 'https://login.example.test/', false, true)
    await vi.advanceTimersByTimeAsync(2_000)
    expect(settled).not.toHaveBeenCalled()
    guest.emit('did-fail-load', {}, -3, 'cancelled for redirect', '', true)
    await Promise.resolve()
    expect(settled).not.toHaveBeenCalled()
    guest.emit('did-finish-load')
    await lifecycle.settled
    expect(guest.eventNames()).toEqual([])
    expect(vi.getTimerCount()).toBe(0)
  })

  it('bounds a genuinely blank popup after admission and cleans up if closed earlier', async () => {
    vi.useFakeTimers()
    const guest = new EventEmitter()
    const lifecycle = observeNativePopupLoad(guest as unknown as WebContents)
    lifecycle.start()
    await vi.advanceTimersByTimeAsync(1_000)
    await lifecycle.settled
    expect(guest.eventNames()).toEqual([])
    const closed = observeNativePopupLoad(guest as unknown as WebContents)
    closed.start()
    guest.emit('destroyed')
    await closed.settled
    expect(guest.eventNames()).toEqual([])
    expect(vi.getTimerCount()).toBe(0)
  })
})
