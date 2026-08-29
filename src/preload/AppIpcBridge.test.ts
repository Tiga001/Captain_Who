import type { IpcRenderer } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createAppIpcBridge } from './AppIpcBridge'

describe('AppIpcBridge', () => {
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
})
