import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createHumanInteractionIpcBridge } from './HumanInteractionIpcBridge'

const fixture = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/human-interaction-v1.json'),
    'utf8'
  )
)

describe('Human interaction Preload bridge', () => {
  it('uses the fixed namespace and preserves invocation success/failure envelopes', async () => {
    const envelope = { ok: true, value: fixture.settings }
    const failure = {
      ok: false,
      error: { message: 'Request already settled', code: -32000, data: { code: 'already_settled' } }
    }
    const invoke = vi.fn().mockResolvedValue(envelope)
    const bridge = createHumanInteractionIpcBridge({ invoke, on: vi.fn(), removeListener: vi.fn() })
    await expect(bridge.getSettings({})).resolves.toEqual(envelope)
    await bridge.updateSettings(fixture.settingsUpdate)
    await bridge.listRequests(fixture.listInput)
    await bridge.submit(fixture.submit)
    invoke.mockResolvedValueOnce(failure)
    await expect(bridge.ignore(fixture.ignore)).resolves.toEqual(failure)
    expect(invoke.mock.calls).toEqual([
      [HOST_CHANNELS.humanInteraction.getSettings, {}],
      [HOST_CHANNELS.humanInteraction.updateSettings, fixture.settingsUpdate],
      [HOST_CHANNELS.humanInteraction.listRequests, fixture.listInput],
      [HOST_CHANNELS.humanInteraction.submit, fixture.submit],
      [HOST_CHANNELS.humanInteraction.ignore, fixture.ignore]
    ])
  })

  it('drops malformed push payloads and removes the exact listeners on unsubscribe', () => {
    const listeners = new Map<string, (...args: unknown[]) => void>()
    const removeListener = vi.fn()
    const bridge = createHumanInteractionIpcBridge({
      invoke: vi.fn(),
      on: vi.fn((channel, listener) => {
        listeners.set(channel, listener)
        return undefined as never
      }),
      removeListener
    })
    const settings = vi.fn(),
      requests = vi.fn()
    const stopSettings = bridge.onSettingsChanged(settings),
      stopRequests = bridge.onRequestChanged(requests)
    listeners.get(HOST_CHANNELS.humanInteraction.settingsChanged)?.({}, fixture.settings)
    listeners.get(HOST_CHANNELS.humanInteraction.settingsChanged)?.(
      {},
      { ...fixture.settings, revision: 0.5 }
    )
    listeners.get(HOST_CHANNELS.humanInteraction.requestChanged)?.({}, fixture.request)
    listeners.get(HOST_CHANNELS.humanInteraction.requestChanged)?.(
      {},
      { ...fixture.request, internalCheckpoint: 'secret' }
    )
    expect(settings).toHaveBeenCalledExactlyOnceWith(fixture.settings)
    expect(requests).toHaveBeenCalledExactlyOnceWith(fixture.request)
    stopSettings()
    stopRequests()
    expect(removeListener).toHaveBeenCalledWith(
      HOST_CHANNELS.humanInteraction.settingsChanged,
      listeners.get(HOST_CHANNELS.humanInteraction.settingsChanged)
    )
    expect(removeListener).toHaveBeenCalledWith(
      HOST_CHANNELS.humanInteraction.requestChanged,
      listeners.get(HOST_CHANNELS.humanInteraction.requestChanged)
    )
  })
})
