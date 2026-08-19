import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  BROWSER_ARTIFACT_SCHEMA_VERSION,
  parseBrowserArtifactReadInput,
  parseBrowserArtifactReadOutput
} from '@mycopilot/protocol'

import type { BrowserArtifactBroker } from '../browser/BrowserArtifactBroker'
import type { TrustedIpcMain } from './trustedIpc'

/** Renderer can resolve only an exact frozen reference; paths are never accepted as locators. */
export function registerBrowserArtifactIpc(
  ipcMain: TrustedIpcMain,
  broker: BrowserArtifactBroker
): void {
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
