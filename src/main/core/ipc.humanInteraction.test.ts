import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import type { IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'
const { getAllWindows, handle } = vi.hoisted(() => ({ getAllWindows: vi.fn(), handle: vi.fn() }))
vi.mock('electron', () => ({ BrowserWindow: { getAllWindows }, ipcMain: { handle, on: vi.fn() } }))
import { registerHumanInteractionIpc } from '../ipc/humanInteractionIpc'
import { createTrustedIpcMain } from '../ipc/trustedIpc'

const fixture = JSON.parse(
  readFileSync(
    resolve(process.cwd(), 'packages/protocol/fixtures/human-interaction-v1.json'),
    'utf8'
  )
)
function core() {
  return {
    getHumanInteractionSettings: vi.fn().mockResolvedValue(fixture.settings),
    updateHumanInteractionSettings: vi.fn().mockResolvedValue(fixture.settingsUpdated),
    listHumanInteractionRequests: vi
      .fn()
      .mockResolvedValue({ items: [fixture.request], nextCursor: null }),
    submitHumanInteractionRequest: vi.fn(),
    ignoreHumanInteractionRequest: vi.fn(),
    onHumanInteractionSettingsChanged: vi.fn().mockReturnValue(vi.fn()),
    onHumanInteractionRequestChanged: vi.fn().mockReturnValue(vi.fn()),
    onCollaborationResync: vi.fn().mockReturnValue(vi.fn())
  }
}
function handler(channel: string): (event: IpcMainInvokeEvent, input: unknown) => Promise<unknown> {
  return handle.mock.calls.find(([name]) => name === channel)![1]
}
beforeEach(() => {
  handle.mockReset()
  getAllWindows.mockReset().mockReturnValue([])
})

describe('Trusted human interaction IPC', () => {
  it('awaits the current Core lease before submitting and preserves a structured access denial', async () => {
    const server = core()
    let synchronized = false
    const sync = vi.fn(async () => {
      synchronized = true
    })
    server.submitHumanInteractionRequest.mockImplementation(async () => {
      expect(synchronized).toBe(true)
      throw Object.assign(new Error('Sign in to start a new turn'), {
        code: -32047,
        data: { type: 'human_interaction_error', code: 'ACCOUNT_LOGIN_REQUIRED' }
      })
    })
    registerHumanInteractionIpc(
      createTrustedIpcMain(() => true),
      server as never,
      sync
    )
    await expect(
      handler(HOST_CHANNELS.humanInteraction.submit)({} as never, fixture.submit)
    ).resolves.toMatchObject({
      ok: false,
      error: { data: { type: 'human_interaction_error', code: 'ACCOUNT_LOGIN_REQUIRED' } }
    })
    expect(sync).toHaveBeenCalledOnce()
  })
  it('registers five authenticated calls and wraps real Core results and errors', async () => {
    const server = core()
    registerHumanInteractionIpc(
      createTrustedIpcMain(() => true),
      server as never
    )
    const event = {} as IpcMainInvokeEvent
    await expect(handler(HOST_CHANNELS.humanInteraction.getSettings)(event, {})).resolves.toEqual({
      ok: true,
      value: fixture.settings
    })
    await expect(
      handler(HOST_CHANNELS.humanInteraction.listRequests)(event, fixture.listInput)
    ).resolves.toEqual({ ok: true, value: { items: [fixture.request], nextCursor: null } })
    await expect(
      handler(HOST_CHANNELS.humanInteraction.updateSettings)(event, fixture.settingsUpdate)
    ).resolves.toEqual({ ok: true, value: fixture.settingsUpdated })
    const error = Object.assign(new Error('Question request not found'), {
      code: -32047,
      data: { type: 'human_interaction_error', code: 'request_not_found' }
    })
    server.submitHumanInteractionRequest.mockRejectedValue(error)
    server.ignoreHumanInteractionRequest.mockRejectedValue(error)
    for (const [channel, input] of [
      [HOST_CHANNELS.humanInteraction.submit, fixture.submit],
      [HOST_CHANNELS.humanInteraction.ignore, fixture.ignore]
    ] as const) {
      await expect(handler(channel)(event, input)).resolves.toEqual({
        ok: false,
        error: { message: error.message, code: error.code, data: error.data }
      })
    }
    expect(handle).toHaveBeenCalledTimes(5)
  })

  it('rejects invalid fields without calling Core and blocks untrusted senders', async () => {
    const server = core()
    const trusted = vi.fn().mockReturnValue(true)
    registerHumanInteractionIpc(createTrustedIpcMain(trusted), server as never)
    await expect(
      handler(HOST_CHANNELS.humanInteraction.ignore)({} as never, {
        ...fixture.ignore,
        answers: []
      })
    ).resolves.toMatchObject({ ok: false })
    expect(server.ignoreHumanInteractionRequest).not.toHaveBeenCalled()
    trusted.mockReturnValue(false)
    expect(() => handler(HOST_CHANNELS.humanInteraction.getSettings)({} as never, {})).toThrow(
      'Blocked untrusted'
    )
    expect(server.getHumanInteractionSettings).not.toHaveBeenCalled()
  })

  it('broadcasts validated updates only to live windows and cleans up subscriptions once', () => {
    const server = core()
    const send = vi.fn(),
      destroyedSend = vi.fn()
    getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } },
      { isDestroyed: () => true, webContents: { isDestroyed: () => false, send: destroyedSend } }
    ])
    const dispose = registerHumanInteractionIpc(
      createTrustedIpcMain(() => true),
      server as never
    )
    server.onHumanInteractionSettingsChanged.mock.calls[0][0](fixture.settings)
    server.onHumanInteractionRequestChanged.mock.calls[0][0](fixture.request)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    server.onHumanInteractionRequestChanged.mock.calls[0][0]({
      ...fixture.request,
      privateKey: true
    })
    expect(send.mock.calls).toEqual([
      [HOST_CHANNELS.humanInteraction.settingsChanged, fixture.settings],
      [HOST_CHANNELS.humanInteraction.requestChanged, fixture.request]
    ])
    expect(destroyedSend).not.toHaveBeenCalled()
    dispose()
    dispose()
    expect(server.onHumanInteractionSettingsChanged.mock.results[0].value).toHaveBeenCalledOnce()
    expect(server.onHumanInteractionRequestChanged.mock.results[0].value).toHaveBeenCalledOnce()
    warn.mockRestore()
  })
})

it('forwards Core reconnect as an independent human interaction refresh hint', () => {
  const server = core()
  const send = vi.fn()
  getAllWindows.mockReturnValue([
    { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } }
  ])
  const dispose = registerHumanInteractionIpc(
    createTrustedIpcMain(() => true),
    server as never
  )
  server.onCollaborationResync.mock.calls[0][0]({})
  expect(send).toHaveBeenCalledWith(HOST_CHANNELS.humanInteraction.resync, null)
  dispose()
  expect(server.onCollaborationResync.mock.results[0].value).toHaveBeenCalledOnce()
})
