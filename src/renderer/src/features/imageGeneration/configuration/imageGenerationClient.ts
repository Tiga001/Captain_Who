// Renderer image-generation configuration client: unwraps structured HostInvocationResult values.
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  ImageGenerationGetConfigurationOutput,
  ImageGenerationSetEnabledInput,
  ImageGenerationSetEnabledOutput,
  ImageGenerationUpdateConfigurationInput,
  ImageGenerationUpdateConfigurationOutput
} from '@mycopilot/protocol'
import {
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationSetEnabledInput,
  parseImageGenerationSetEnabledOutput,
  parseImageGenerationUpdateConfigurationInput,
  parseImageGenerationUpdateConfigurationOutput
} from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'

export async function getImageGenerationConfiguration(): Promise<ImageGenerationGetConfigurationOutput> {
  const result = await hostClient.imageGeneration.getConfiguration()
  if (!result.ok) throw new HostInvocationError(result.error)
  return parseImageGenerationGetConfigurationOutput(result.value)
}

export async function updateImageGenerationConfiguration(
  input: ImageGenerationUpdateConfigurationInput
): Promise<ImageGenerationUpdateConfigurationOutput> {
  const result = await hostClient.imageGeneration.updateConfiguration(
    parseImageGenerationUpdateConfigurationInput(input)
  )
  if (!result.ok) throw new HostInvocationError(result.error)
  return parseImageGenerationUpdateConfigurationOutput(result.value)
}

export async function setImageGenerationEnabled(
  input: ImageGenerationSetEnabledInput
): Promise<ImageGenerationSetEnabledOutput> {
  const result = await hostClient.imageGeneration.setEnabled(
    parseImageGenerationSetEnabledInput(input)
  )
  if (!result.ok) throw new HostInvocationError(result.error)
  return parseImageGenerationSetEnabledOutput(result.value)
}
