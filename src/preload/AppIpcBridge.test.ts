import type { IpcRenderer } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createAppIpcBridge } from './AppIpcBridge'

describe('AppIpcBridge', () => {
  it.each(['showAbout', 'openDocumentation'] as const)(
    'invokes %s without forwarding renderer arguments',
    async (method) => {
      const invoke = vi.fn(async () => undefined)
      const bridge = createAppIpcBridge({
        invoke,
        on: vi.fn(),
        removeListener: vi.fn(),
        send: vi.fn()
      } as unknown as IpcRenderer)

      await bridge[method]()
      await Reflect.apply(bridge[method], undefined, ['https://example.com', { activate: false }])

      expect(invoke.mock.calls).toEqual([[HOST_CHANNELS.app[method]], [HOST_CHANNELS.app[method]]])
    }
  )

  it('waits on the dedicated startup readiness channel without arguments', async () => {
    const invoke = vi.fn(async () => undefined)
    const bridge = createAppIpcBridge({
      invoke,
      on: vi.fn(),
      removeListener: vi.fn()
    } as unknown as IpcRenderer)

    await bridge.whenReady()

    expect(invoke).toHaveBeenCalledWith(HOST_CHANNELS.app.whenReady)
  })

  it('acknowledges a quit flush only after the Renderer handler settles', async () => {
    let listener: ((event: unknown, requestId: string) => void) | undefined
    let finishFlush: (() => void) | undefined
    const send = vi.fn()
    const removeListener = vi.fn()
    const bridge = createAppIpcBridge({
      invoke: vi.fn(),
      on: vi.fn((channel, nextListener) => {
        if (channel === HOST_CHANNELS.app.flushBeforeQuit)
          listener = nextListener as typeof listener
        return {} as IpcRenderer
      }),
      removeListener,
      send
    } as unknown as IpcRenderer)
    const unsubscribe = bridge.onFlushBeforeQuit(
      () =>
        new Promise<void>((resolve) => {
          finishFlush = resolve
        })
    )

    listener?.({}, 'quit-1')
    await Promise.resolve()
    expect(send).not.toHaveBeenCalled()

    finishFlush?.()
    await vi.waitFor(() =>
      expect(send).toHaveBeenCalledWith(HOST_CHANNELS.app.flushBeforeQuitAck, 'quit-1')
    )
    unsubscribe()
    expect(removeListener).toHaveBeenCalledWith(HOST_CHANNELS.app.flushBeforeQuit, listener)
  })

  it('acknowledges only after every registered Renderer persistence handler settles', async () => {
    let listener: ((event: unknown, requestId: string) => void) | undefined
    let finishMessages: (() => void) | undefined
    let finishDrafts: (() => void) | undefined
    const send = vi.fn()
    const on = vi.fn((channel, nextListener) => {
      if (channel === HOST_CHANNELS.app.flushBeforeQuit) listener = nextListener as typeof listener
      return {} as IpcRenderer
    })
    const bridge = createAppIpcBridge({
      invoke: vi.fn(),
      on,
      removeListener: vi.fn(),
      send
    } as unknown as IpcRenderer)

    bridge.onFlushBeforeQuit(
      () =>
        new Promise<void>((resolve) => {
          finishMessages = resolve
        })
    )
    bridge.onFlushBeforeQuit(
      () =>
        new Promise<void>((resolve) => {
          finishDrafts = resolve
        })
    )
    expect(on).toHaveBeenCalledTimes(1)

    listener?.({}, 'quit-all')
    await vi.waitFor(() => {
      expect(finishMessages).toBeTypeOf('function')
      expect(finishDrafts).toBeTypeOf('function')
    })
    finishMessages?.()
    await Promise.resolve()
    expect(send).not.toHaveBeenCalled()

    finishDrafts?.()
    await vi.waitFor(() =>
      expect(send).toHaveBeenCalledWith(HOST_CHANNELS.app.flushBeforeQuitAck, 'quit-all')
    )
  })
})
