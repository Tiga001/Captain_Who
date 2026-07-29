import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type ImageGenerationHostApi } from '@mycopilot/host-api'

export const IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL =
  HOST_CHANNELS.imageGeneration.getConfiguration
export const IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL =
  HOST_CHANNELS.imageGeneration.updateConfiguration
export const IMAGE_GENERATION_SET_ENABLED_CHANNEL = HOST_CHANNELS.imageGeneration.setEnabled
export const IMAGE_GENERATION_GET_STATUS_CHANNEL = HOST_CHANNELS.imageGeneration.getStatus
export const IMAGE_GENERATION_READ_ARTIFACT_CHANNEL = HOST_CHANNELS.imageGeneration.readArtifact

type ImageGenerationIpcRenderer = Pick<IpcRenderer, 'invoke'>

/**
 * Transport-only preload bridge. Main owns the Host invocation envelope and CoreServer owns
 * strict request/response validation, so secret-bearing configuration updates are never
 * interpreted or logged in the renderer-facing preload process.
 */
export function createImageGenerationIpcBridge(
  ipcRenderer: ImageGenerationIpcRenderer
): ImageGenerationHostApi {
  return {
    getConfiguration: () => ipcRenderer.invoke(IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL),
    updateConfiguration: (input) =>
      ipcRenderer.invoke(IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL, input),
    setEnabled: (input) => ipcRenderer.invoke(IMAGE_GENERATION_SET_ENABLED_CHANNEL, input),
    getStatus: () => ipcRenderer.invoke(IMAGE_GENERATION_GET_STATUS_CHANNEL),
    readArtifact: (input) => ipcRenderer.invoke(IMAGE_GENERATION_READ_ARTIFACT_CHANNEL, input)
  }
}
