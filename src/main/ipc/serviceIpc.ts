import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export function registerCoreServiceIpc(ipcMain: TrustedIpcMain, coreServer: CoreServer): void {
  ipcMain.handle(HOST_CHANNELS.core.ping, (_event, input) => coreServer.ping(input))
  ipcMain.handle(HOST_CHANNELS.office.getStatus, () => coreServer.getOfficeStatus())
  ipcMain.handle(HOST_CHANNELS.imageGeneration.getConfiguration, () =>
    captureHostInvocation(() => coreServer.getImageGenerationConfiguration())
  )
  ipcMain.handle(HOST_CHANNELS.imageGeneration.updateConfiguration, (_event, input) =>
    captureHostInvocation(() => coreServer.updateImageGenerationConfiguration(input))
  )
  ipcMain.handle(HOST_CHANNELS.imageGeneration.setEnabled, (_event, input) =>
    captureHostInvocation(() => coreServer.setImageGenerationEnabled(input))
  )
  ipcMain.handle(HOST_CHANNELS.imageGeneration.getStatus, () =>
    captureHostInvocation(() => coreServer.getImageGenerationStatus())
  )
  ipcMain.handle(HOST_CHANNELS.imageGeneration.readArtifact, (_event, input) =>
    captureHostInvocation(() => coreServer.readImageGenerationArtifact(input))
  )
  ipcMain.handle(HOST_CHANNELS.search.searchChats, (_event, input) => coreServer.searchChats(input))
}
