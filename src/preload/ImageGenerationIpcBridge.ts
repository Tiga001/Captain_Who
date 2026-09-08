import type { IpcRenderer } from 'electron'
import {
  HOST_CHANNELS,
  type HostInvocationResult,
  type ImageGenerationHostApi
} from '@mycopilot/host-api'
import {
  parseImageGenerationArtifactReadInput,
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationSetEnabledInput,
  parseImageGenerationSetEnabledOutput,
  parseImageGenerationStatus,
  parseImageGenerationUpdateConfigurationInput,
  parseImageGenerationUpdateConfigurationOutput
} from '@mycopilot/protocol'

export const IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL =
  HOST_CHANNELS.imageGeneration.getConfiguration
export const IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL =
  HOST_CHANNELS.imageGeneration.updateConfiguration
export const IMAGE_GENERATION_SET_ENABLED_CHANNEL = HOST_CHANNELS.imageGeneration.setEnabled
export const IMAGE_GENERATION_GET_STATUS_CHANNEL = HOST_CHANNELS.imageGeneration.getStatus
export const IMAGE_GENERATION_READ_ARTIFACT_CHANNEL = HOST_CHANNELS.imageGeneration.readArtifact

type ImageGenerationIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

async function parseSuccessfulInvocation<T>(
  invocation: Promise<HostInvocationResult<unknown>>,
  parseValue: (value: unknown) => T
): Promise<HostInvocationResult<T>> {
  const result = await invocation
  return result.ok
    ? { ok: true, value: parseValue(result.value) }
    : (result as HostInvocationResult<T>)
}

/**
 * The isolated bridge validates both mutation requests and successful Host projections. This
 * deliberately duplicates the Core boundary so an accidental legacy secret field cannot reach
 * Renderer even if an upstream regression reintroduces it.
 */
export function createImageGenerationIpcBridge(
  ipcRenderer: ImageGenerationIpcRenderer
): ImageGenerationHostApi {
  return {
    onChanged: (handler) => {
      const listener = (): void => handler()
      ipcRenderer.on(HOST_CHANNELS.imageGeneration.changed, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.imageGeneration.changed, listener)
    },
    getConfiguration: () =>
      parseSuccessfulInvocation(
        ipcRenderer.invoke(IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL),
        parseImageGenerationGetConfigurationOutput
      ),
    updateConfiguration: (input) =>
      parseSuccessfulInvocation(
        ipcRenderer.invoke(
          IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL,
          parseImageGenerationUpdateConfigurationInput(input)
        ),
        parseImageGenerationUpdateConfigurationOutput
      ),
    setEnabled: (input) =>
      parseSuccessfulInvocation(
        ipcRenderer.invoke(
          IMAGE_GENERATION_SET_ENABLED_CHANNEL,
          parseImageGenerationSetEnabledInput(input)
        ),
        parseImageGenerationSetEnabledOutput
      ),
    getStatus: () =>
      parseSuccessfulInvocation(
        ipcRenderer.invoke(IMAGE_GENERATION_GET_STATUS_CHANNEL),
        parseImageGenerationStatus
      ),
    readArtifact: (input) =>
      ipcRenderer.invoke(
        IMAGE_GENERATION_READ_ARTIFACT_CHANNEL,
        parseImageGenerationArtifactReadInput(input)
      )
  }
}
