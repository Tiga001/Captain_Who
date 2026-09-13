import { EventEmitter } from 'node:events'
import type { IpcRenderer } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS, type UpdateState } from '@mycopilot/host-api'
import { createUpdateIpcBridge } from './UpdateIpcBridge'

const available: UpdateState = {
  revision: 1,
  status: 'available',
  version: '1.1.0',
  percent: 0,
  error: null
}

function fixture() {
  const events = new EventEmitter()
  const invoke = vi.fn()
  const on = vi.fn((channel: string, listener: (...args: unknown[]) => void) =>
    events.on(channel, listener)
  )
  const removeListener = vi.fn((channel: string, listener: (...args: unknown[]) => void) =>
    events.removeListener(channel, listener)
  )
  const send = vi.fn()
  const bridge = createUpdateIpcBridge({
    invoke,
    on,
    removeListener,
    send
  } as unknown as IpcRenderer)
  return { bridge, invoke, on, removeListener, send, events }
}

describe('desktop update preload bridge', () => {
  it('exposes only getState, download and onStateChanged, with no source or install controls', () => {
    const { bridge, invoke, send } = fixture()
    expect(Object.keys(bridge).sort()).toEqual(['download', 'getState', 'onStateChanged'])
    for (const name of [
      'check',
      'checkForUpdates',
      'setFeedURL',
      'feedUrl',
      'url',
      'configure',
      'config',
      'install',
      'quitAndInstall',
      'cancel',
      'autoDownload',
      'ipcRenderer'
    ])
      expect(bridge).not.toHaveProperty(name)
    expect(invoke).not.toHaveBeenCalled()
    expect(send).not.toHaveBeenCalled()
  })

  it('uses exactly two fixed invoke channels without forwarding renderer-supplied arguments', async () => {
    const { bridge, invoke } = fixture()
    const started: UpdateState = { ...available, revision: 2, status: 'downloading' }
    invoke.mockResolvedValueOnce(available).mockResolvedValueOnce(started)

    await expect(
      Reflect.apply(bridge.getState, null, [{ includeCredentials: true }])
    ).resolves.toEqual(available)
    await expect(
      Reflect.apply(bridge.download, null, [
        'https://untrusted.example/update.zip',
        { install: false }
      ])
    ).resolves.toEqual(started)
    expect(invoke.mock.calls).toEqual([
      [HOST_CHANNELS.updates.getState],
      [HOST_CHANNELS.updates.download]
    ])
  })

  it('keeps invocation failures observable so the renderer can handle an unavailable host', async () => {
    const { bridge, invoke } = fixture()
    const failure = new Error('Host unavailable')
    invoke.mockRejectedValue(failure)
    await expect(bridge.getState()).rejects.toBe(failure)
    await expect(bridge.download()).rejects.toBe(failure)
  })

  it('delivers only the public state argument and removes the exact listener for each subscription', () => {
    const { bridge, on, removeListener, events, send } = fixture()
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = bridge.onStateChanged(first)
    const stopSecond = bridge.onStateChanged(second)
    const event = { sender: { privileged: true }, ports: [{ privatePort: true }] }
    const state: UpdateState = { ...available, revision: 2, status: 'downloading', percent: 38 }

    events.emit(HOST_CHANNELS.updates.stateChanged, event, state, { unexpected: true })
    expect(first).toHaveBeenCalledExactlyOnceWith(state)
    expect(second).toHaveBeenCalledExactlyOnceWith(state)
    expect(first.mock.calls[0]).not.toContain(event)
    expect(on.mock.calls.map(([channel]) => channel)).toEqual([
      HOST_CHANNELS.updates.stateChanged,
      HOST_CHANNELS.updates.stateChanged
    ])
    expect(send).not.toHaveBeenCalled()
    stopFirst()
    expect(removeListener).toHaveBeenCalledExactlyOnceWith(
      HOST_CHANNELS.updates.stateChanged,
      on.mock.calls[0][1]
    )
    events.emit(HOST_CHANNELS.updates.stateChanged, event, { ...state, revision: 3, percent: 80 })
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledTimes(2)
    stopSecond()
    expect(removeListener).toHaveBeenNthCalledWith(
      2,
      HOST_CHANNELS.updates.stateChanged,
      on.mock.calls[1][1]
    )
    events.emit(HOST_CHANNELS.updates.stateChanged, event, { ...state, revision: 4, percent: 100 })
    expect(second).toHaveBeenCalledTimes(2)
    expect(events.listenerCount(HOST_CHANNELS.updates.stateChanged)).toBe(0)
  })
})
