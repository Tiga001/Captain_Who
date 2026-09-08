import { BrowserWindow } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { HostConfigurationDomain } from '../core/coreServerStorageApi'

/** Main-process invalidations carry neither configuration, credentials nor RPC responses. */
export function registerConfigurationNotifications(
  coreServer: Pick<CoreServer, 'onConfigurationInvalidated'>
): () => void {
  const channels: Record<HostConfigurationDomain, string> = {
    modelSettings: HOST_CHANNELS.storage.modelSettingsChanged,
    imageGeneration: HOST_CHANNELS.imageGeneration.changed,
    builtinCapabilities: HOST_CHANNELS.mcp.builtinCapabilitiesChanged
  }
  const unsubscribers = Object.entries(channels).map(([domain, channel]) =>
    coreServer.onConfigurationInvalidated(domain as HostConfigurationDomain, () => {
      for (const window of BrowserWindow.getAllWindows()) {
        if (window.isDestroyed() || window.webContents.isDestroyed()) continue
        try {
          window.webContents.send(channel)
        } catch {
          // A window may disappear between enumeration and delivery; other windows still refresh.
        }
      }
    })
  )
  return () => unsubscribers.forEach((unsubscribe) => unsubscribe())
}
