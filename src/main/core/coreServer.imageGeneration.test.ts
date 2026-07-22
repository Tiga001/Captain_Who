import type {
  ImageGenerationConfiguration,
  ImageGenerationSetEnabledInput,
  ImageGenerationUpdateConfigurationInput
} from '@mycopilot/protocol'
import {
  IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
  IMAGE_GENERATION_GET_CONFIGURATION_METHOD,
  IMAGE_GENERATION_GET_STATUS_METHOD,
  IMAGE_GENERATION_SET_ENABLED_METHOD,
  IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

const configuration = {
  adapterId: 'smartmlSeedream',
  endpointUrl: 'https://example.test/userapi/v1/images/generations',
  modelId: 'doubao-seedream-4-0-250828',
  capabilities: { textToImage: true, imageToImage: true },
  defaults: { sizePreset: '2K', watermark: true },
  credentialStatus: 'configured',
  enabled: true,
  readiness: 'readyUnverified',
  revision: 'image-generation:v1:1'
} satisfies ImageGenerationConfiguration

const updateInput = {
  schemaVersion: 1,
  expectedRevision: configuration.revision,
  adapterId: 'smartmlSeedream',
  endpointUrl: configuration.endpointUrl,
  modelId: configuration.modelId,
  capabilities: configuration.capabilities,
  defaults: configuration.defaults,
  credentialMutation: { type: 'keep' }
} satisfies ImageGenerationUpdateConfigurationInput

const setEnabledInput = {
  schemaVersion: 1,
  expectedRevision: configuration.revision,
  enabled: false
} satisfies ImageGenerationSetEnabledInput

describe('CoreServer image generation configuration client', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('uses stable methods and validates every successful response', async () => {
    const getOutput = { schemaVersion: 1, configuration }
    const updateOutput = { schemaVersion: 1, outcome: 'alreadyCurrent', configuration } as const
    const disabledConfiguration = {
      ...configuration,
      enabled: false,
      readiness: 'disabled' as const,
      revision: 'image-generation:v1:2'
    }
    const setEnabledOutput = {
      schemaVersion: 1,
      outcome: 'updated',
      configuration: disabledConfiguration
    } as const
    const status = {
      schemaVersion: 1,
      adapterId: configuration.adapterId,
      configurationRevision: configuration.revision,
      enabled: true,
      readiness: configuration.readiness,
      credentialStatus: configuration.credentialStatus,
      capabilities: configuration.capabilities,
      defaults: configuration.defaults
    } as const
    rpcRequest
      .mockResolvedValueOnce(getOutput)
      .mockResolvedValueOnce(updateOutput)
      .mockResolvedValueOnce(setEnabledOutput)
      .mockResolvedValueOnce(status)
    const server = new CoreServer()

    await expect(server.getImageGenerationConfiguration()).resolves.toEqual(getOutput)
    await expect(server.updateImageGenerationConfiguration(updateInput)).resolves.toEqual(
      updateOutput
    )
    await expect(server.setImageGenerationEnabled(setEnabledInput)).resolves.toEqual(
      setEnabledOutput
    )
    await expect(server.getImageGenerationStatus()).resolves.toEqual(status)

    expect(rpcRequest.mock.calls).toEqual([
      [IMAGE_GENERATION_GET_CONFIGURATION_METHOD],
      [IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD, updateInput],
      [IMAGE_GENERATION_SET_ENABLED_METHOD, setEnabledInput],
      [IMAGE_GENERATION_GET_STATUS_METHOD]
    ])
  })

  it('rejects malformed input before any secret can cross the JSON-RPC boundary', () => {
    const invalid = {
      ...updateInput,
      credentialMutation: { type: 'replace', value: ' key-with-surrounding-space ' }
    }

    expect(() => new CoreServer().updateImageGenerationConfiguration(invalid as never)).toThrow(
      'credentialMutation.value'
    )
    expect(rpcRequest).not.toHaveBeenCalled()
  })

  it('rejects malformed successful responses', async () => {
    rpcRequest.mockResolvedValue({
      schemaVersion: 1,
      configuration: { ...configuration, credential: 'must-never-cross-the-boundary' }
    })

    await expect(new CoreServer().getImageGenerationConfiguration()).rejects.toThrow(
      'Image generation configuration response.configuration'
    )
  })

  it('preserves only validated structured configuration error data', async () => {
    const data = {
      type: 'imageGenerationConfiguration',
      operation: 'updateConfiguration',
      code: 'revisionConflict',
      recovery: 'refreshConfiguration',
      message: 'The configuration changed. Refresh and try again.',
      currentRevision: 'image-generation:v1:2'
    } as const
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('provider failure: Authorization: Bearer must-not-cross-boundary'), {
        code: IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
        data
      })
    )

    await expect(
      new CoreServer().updateImageGenerationConfiguration(updateInput)
    ).rejects.toMatchObject({
      message: data.message,
      code: IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
      data
    })
  })

  it('rejects an unvalidated error payload instead of forwarding it', async () => {
    rpcRequest.mockRejectedValueOnce(
      Object.assign(new Error('unsafe error'), {
        code: IMAGE_GENERATION_CONFIGURATION_ERROR_CODE,
        data: {
          type: 'imageGenerationConfiguration',
          operation: 'updateConfiguration',
          code: 'providerDump',
          apiKey: 'must-not-cross-the-boundary'
        }
      })
    )

    await expect(new CoreServer().updateImageGenerationConfiguration(updateInput)).rejects.toThrow(
      'Image generation configuration error data'
    )
  })
})
