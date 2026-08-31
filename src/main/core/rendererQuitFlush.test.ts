import type { IpcMainEvent, WebContents } from 'electron'
import { EventEmitter } from 'node:events'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { TrustedIpcMain } from '../ipc/trustedIpc'
import { RendererQuitFlushCoordinator } from '../ipc/rendererQuitFlush'

function rendererTarget() {
  const events = new EventEmitter()
  const send = vi.fn()
  const target = {
    isDestroyed: () => false,
    once: events.once.bind(events),
    removeListener: events.removeListener.bind(events),
    send
  } as unknown as WebContents
  return { events, send, target }
}

describe('RendererQuitFlushCoordinator', () => {
  it('accepts an acknowledgement only from the requested Renderer and exact request id', async () => {
    let acknowledge: ((event: IpcMainEvent, requestId: unknown) => void) | undefined
    const ipcMain = {
      handle: vi.fn(),
      on: vi.fn((channel, listener) => {
        if (channel === HOST_CHANNELS.app.flushBeforeQuitAck) acknowledge = listener
      })
    } as unknown as TrustedIpcMain
    const coordinator = new RendererQuitFlushCoordinator(ipcMain)
    const { send, target } = rendererTarget()
    const { target: spoofedTarget } = rendererTarget()

    const flushing = coordinator.flush(target, 5_000)
    const requestId = send.mock.calls[0]?.[1] as string
    let settled = false
    void flushing.then(() => {
      settled = true
    })

    acknowledge?.({ sender: spoofedTarget } as IpcMainEvent, requestId)
    await Promise.resolve()
    expect(settled).toBe(false)

    acknowledge?.({ sender: target } as IpcMainEvent, requestId)
    await flushing
    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.app.flushBeforeQuit, requestId)
    coordinator.dispose()
  })

  it('fails open when the target Renderer is destroyed', async () => {
    const ipcMain = { handle: vi.fn(), on: vi.fn() } as unknown as TrustedIpcMain
    const coordinator = new RendererQuitFlushCoordinator(ipcMain)
    const { events, target } = rendererTarget()

    const flushing = coordinator.flush(target, 5_000)
    events.emit('destroyed')

    await expect(flushing).resolves.toBeUndefined()
    coordinator.dispose()
  })
})
