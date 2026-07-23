// Renderer image-generation configuration client: unwraps structured HostInvocationResult values.
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  ImageGenerationGetConfigurationOutput,
  ImageGenerationSetEnabledInput,
  ImageGenerationSetEnabledOutput,
  ImageGenerationStatus,
  ImageGenerationUpdateConfigurationInput,
  ImageGenerationUpdateConfigurationOutput
} from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'

export async function getImageGenerationConfiguration(): Promise<ImageGenerationGetConfigurationOutput> {
  const result = await hostClient.imageGeneration.getConfiguration()
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function updateImageGenerationConfiguration(
  input: ImageGenerationUpdateConfigurationInput
): Promise<ImageGenerationUpdateConfigurationOutput> {
  const result = await hostClient.imageGeneration.updateConfiguration(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function setImageGenerationEnabled(
  input: ImageGenerationSetEnabledInput
): Promise<ImageGenerationSetEnabledOutput> {
  const result = await hostClient.imageGeneration.setEnabled(input)
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}

export async function getImageGenerationStatus(): Promise<ImageGenerationStatus> {
  const result = await hostClient.imageGeneration.getStatus()
  if (!result.ok) throw new HostInvocationError(result.error)
  return result.value
}
