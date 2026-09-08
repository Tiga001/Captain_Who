import { expect, it, vi } from 'vitest'
import type { HostConfigurationDomain } from './coreServerStorageApi'

const getAllWindows = vi.hoisted(() => vi.fn())
vi.mock('electron', () => ({ BrowserWindow: { getAllWindows } }))

import { registerConfigurationNotifications } from '../ipc/configurationNotifications'

it('broadcasts payload-free configuration invalidations to all live windows and disposes', () => {
  const callbacks = new Map<HostConfigurationDomain, () => void>()
  const unsubscribe = vi.fn()
  const first = vi.fn()
  const second = vi.fn()
  const destroyed = vi.fn()
  const closedContents = vi.fn()
  getAllWindows.mockReturnValue([
    { isDestroyed: () => false, webContents: { isDestroyed: () => false, send: first } },
    { isDestroyed: () => true, webContents: { isDestroyed: () => false, send: destroyed } },
    { isDestroyed: () => false, webContents: { isDestroyed: () => true, send: closedContents } },
    { isDestroyed: () => false, webContents: { isDestroyed: () => false, send: second } }
  ])
  const dispose = registerConfigurationNotifications({
    onConfigurationInvalidated: (domain, callback) => {
      callbacks.set(domain, callback)
      return unsubscribe
    }
  })
  for (const callback of callbacks.values()) callback()
  expect(first.mock.calls).toEqual([
    ['host:storage.modelSettingsChanged'],
    ['host:imageGeneration.changed'],
    ['host:mcp.builtinCapabilitiesChanged']
  ])
  expect(second.mock.calls).toEqual(first.mock.calls)
  expect(destroyed).not.toHaveBeenCalled()
  expect(closedContents).not.toHaveBeenCalled()
  dispose()
  expect(unsubscribe).toHaveBeenCalledTimes(3)
})
