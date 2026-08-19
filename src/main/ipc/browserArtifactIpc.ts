import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import type { IpcMainInvokeEvent } from 'electron'
import {
  BROWSER_ARTIFACT_SCHEMA_VERSION,
  parseBrowserArtifactExportInput,
  parseBrowserArtifactExportOutput,
  parseBrowserArtifactReadInput,
  parseBrowserArtifactReadOutput
} from '@mycopilot/protocol'

import type { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import type { TrustedIpcMain } from './trustedIpc'

export type BrowserArtifactSaveDialog = (
  event: IpcMainInvokeEvent,
  suggestedFileName: string
) => Promise<string | null>

/** Renderer can resolve only an exact frozen reference; paths are never accepted as locators. */
export function registerBrowserArtifactIpc(
  ipcMain: TrustedIpcMain,
  broker: BrowserArtifactBroker,
  showSaveDialog: BrowserArtifactSaveDialog
): void {
  ipcMain.handle(HOST_CHANNELS.browser.artifactExport, (event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserArtifactExportInput(value)
      let destination: string | null
      try {
        destination = await showSaveDialog(event, input.artifact.displayName)
      } catch {
        // Native dialog failures are projected without any platform path or implementation detail.
        throw new Error('browser.artifact.unavailable')
      }
      if (destination === null) {
        return parseBrowserArtifactExportOutput({
          schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
          status: 'cancelled'
        })
      }
      const exported = await broker.exportArtifact(input.artifact, destination)
      return parseBrowserArtifactExportOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        status: 'exported',
        displayName: exported.displayName
      })
    })
  )
  ipcMain.handle(HOST_CHANNELS.browser.artifactReadPreview, (_event, value) =>
    captureHostInvocation(async () => {
      const input = parseBrowserArtifactReadInput(value)
      const preview = await broker.readPreview(input.artifact)
      return parseBrowserArtifactReadOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        artifact: preview.artifact,
        bytes: preview.bytes
      })
    })
  )
}
