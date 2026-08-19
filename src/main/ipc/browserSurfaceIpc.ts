import { HOST_CHANNELS } from '@mycopilot/host-api'
import {
  parseBrowserSurfaceReadyInput,
  parseBrowserSurfaceReadyOutput,
  parseBrowserSurfaceSelectedInput,
  parseBrowserSurfaceSelectedOutput
} from '@mycopilot/protocol'
import type { BrowserSurfaceManager } from '../browser/BrowserSurfaceManager'
import type { TrustedIpcMain } from './trustedIpc'

export function registerBrowserSurfaceIpc(
  ipcMain: TrustedIpcMain,
  manager: BrowserSurfaceManager
): void {
  ipcMain.handle(HOST_CHANNELS.browser.surfaceReady, (event, value) =>
    parseBrowserSurfaceReadyOutput(
      manager.attach(event.sender, parseBrowserSurfaceReadyInput(value))
    )
  )
  ipcMain.handle(HOST_CHANNELS.browser.surfaceSelected, (event, value) =>
    parseBrowserSurfaceSelectedOutput(
      manager.selectManualSurface(event.sender, parseBrowserSurfaceSelectedInput(value))
    )
  )
}
