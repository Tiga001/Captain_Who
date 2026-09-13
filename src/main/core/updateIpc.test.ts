import type { IpcMainInvokeEvent } from 'electron'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS, type UpdateState } from '@mycopilot/host-api'
import type { UpdateService } from '../updates/UpdateService'

const electron = vi.hoisted(() => ({
  handle: vi.fn(),
  on: vi.fn(),
  removeHandler: vi.fn(),
  getAllWindows: vi.fn()
}))
vi.mock('electron', () => ({
  ipcMain: { handle: electron.handle, on: electron.on, removeHandler: electron.removeHandler },
  BrowserWindow: { getAllWindows: electron.getAllWindows }
}))
import { registerUpdateIpc } from '../updates/updateIpc'

type Handler = (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown
const handlers = new Map<string, Handler>()
const available: UpdateState = {
  revision: 1,
  status: 'available',
  version: '1.1.0',
  percent: 0,
  error: null
}
const downloading: UpdateState = { ...available, revision: 2, status: 'downloading' }

function serviceFixture() {
  const listeners = new Set<(state: UpdateState) => void>()
  const unsubscribe = vi.fn()
  const service = {
    getState: vi.fn(() => available),
    download: vi.fn(() => downloading),
    subscribe: vi.fn((handler: (state: UpdateState) => void) => {
      listeners.add(handler)
      return () => {
        unsubscribe()
        listeners.delete(handler)
      }
    })
  }
  return {
    service,
    host: service as unknown as UpdateService,
    unsubscribe,
    publish: (state: UpdateState) => {
      for (const listener of listeners) listener(state)
    }
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  handlers.clear()
  electron.getAllWindows.mockReturnValue([])
  electron.handle.mockImplementation((channel: string, handler: Handler) => {
    handlers.set(channel, handler)
  })
  electron.removeHandler.mockImplementation((channel: string) => {
    handlers.delete(channel)
  })
})

describe('trusted desktop update IPC', () => {
  it('registers only state and explicit download invokes and returns their public states', () => {
    const fixture = serviceFixture()
    const event = { sender: { id: 1 } } as IpcMainInvokeEvent
    const trusted = vi.fn((candidate: IpcMainInvokeEvent) => candidate === event)
    registerUpdateIpc(fixture.host, trusted)

    expect([...handlers.keys()]).toEqual([
      HOST_CHANNELS.updates.getState,
      HOST_CHANNELS.updates.download
    ])
    expect(electron.on).not.toHaveBeenCalled()
    expect(fixture.service.download).not.toHaveBeenCalled()
    expect(handlers.get(HOST_CHANNELS.updates.getState)!(event)).toEqual(available)
    expect(handlers.get(HOST_CHANNELS.updates.download)!(event)).toEqual(downloading)
    expect(fixture.service.getState).toHaveBeenCalledExactlyOnceWith()
    expect(fixture.service.download).toHaveBeenCalledExactlyOnceWith()
    expect(trusted).toHaveBeenCalledTimes(2)
    expect(trusted).toHaveBeenNthCalledWith(1, event)
    expect(trusted).toHaveBeenNthCalledWith(2, event)
  })

  it.each([HOST_CHANNELS.updates.getState, HOST_CHANNELS.updates.download])(
    'rejects an untrusted sender before dispatching %s',
    (channel) => {
      const fixture = serviceFixture()
      const event = { sender: { id: 2 } } as IpcMainInvokeEvent
      const trusted = vi.fn(() => false)
      registerUpdateIpc(fixture.host, trusted)

      expect(() => handlers.get(channel)!(event)).toThrow('Blocked untrusted IPC sender')
      expect(() =>
        handlers.get(channel)!(event, { url: 'https://untrusted.example/update.zip' })
      ).toThrow('Blocked untrusted IPC sender')
      expect(trusted).toHaveBeenCalledWith(event)
      expect(fixture.service.getState).not.toHaveBeenCalled()
      expect(fixture.service.download).not.toHaveBeenCalled()
    }
  )

  it.each([HOST_CHANNELS.updates.getState, HOST_CHANNELS.updates.download])(
    'rejects every supplied argument, including undefined, on %s',
    (channel) => {
      const fixture = serviceFixture()
      registerUpdateIpc(fixture.host, () => true)
      const suppliedArguments: unknown[][] = [
        [undefined],
        [null],
        [{}],
        [0],
        [false],
        ['https://untrusted.example/update.zip'],
        [{ feedUrl: 'https://untrusted.example/feed', autoInstall: true }],
        [undefined, undefined]
      ]

      for (const args of suppliedArguments) {
        expect(() => handlers.get(channel)!({} as IpcMainInvokeEvent, ...args)).toThrow(
          'Update operation does not accept parameters'
        )
      }
      expect(fixture.service.getState).not.toHaveBeenCalled()
      expect(fixture.service.download).not.toHaveBeenCalled()
    }
  )

  it('publishes state only to live windows and removes both invokes and the state subscription', () => {
    const fixture = serviceFixture()
    const send = vi.fn()
    const destroyedWindowSend = vi.fn()
    const destroyedContentsSend = vi.fn()
    electron.getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } },
      {
        isDestroyed: () => true,
        webContents: { isDestroyed: () => false, send: destroyedWindowSend }
      },
      {
        isDestroyed: () => false,
        webContents: { isDestroyed: () => true, send: destroyedContentsSend }
      }
    ])
    const dispose = registerUpdateIpc(fixture.host, () => true)
    const state: UpdateState = { ...downloading, revision: 3, percent: 47 }
    fixture.publish(state)

    expect(send).toHaveBeenCalledExactlyOnceWith(HOST_CHANNELS.updates.stateChanged, state)
    expect(destroyedWindowSend).not.toHaveBeenCalled()
    expect(destroyedContentsSend).not.toHaveBeenCalled()
    dispose()
    expect(fixture.unsubscribe).toHaveBeenCalledOnce()
    expect(electron.removeHandler.mock.calls).toEqual([
      [HOST_CHANNELS.updates.getState],
      [HOST_CHANNELS.updates.download]
    ])
    expect(handlers.size).toBe(0)
    fixture.publish({ ...state, revision: 4, percent: 80 })
    expect(send).toHaveBeenCalledOnce()
  })
})
