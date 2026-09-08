import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  ImageGenerationGetConfigurationOutput,
  ImageGenerationSetEnabledInput,
  ImageGenerationUpdateConfigurationInput
} from '@mycopilot/protocol'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import {
  createImageGenerationIpcBridge,
  IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL,
  IMAGE_GENERATION_GET_STATUS_CHANNEL,
  IMAGE_GENERATION_READ_ARTIFACT_CHANNEL,
  IMAGE_GENERATION_SET_ENABLED_CHANNEL,
  IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL
} from './ImageGenerationIpcBridge'

type ImageGenerationIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

describe('Image generation IPC bridge', () => {
  it('routes all configuration operations without unwrapping the Host invocation result', async () => {
    const invoke = vi.fn()
    const response = {
      ok: false,
      error: {
        message: 'The configuration changed. Refresh and try again.',
        code: -32020,
        data: {
          type: 'imageGenerationConfiguration',
          operation: 'updateConfiguration',
          code: 'revisionConflict',
          recovery: 'refreshConfiguration',
          message: 'The configuration changed. Refresh and try again.',
          currentRevision: 'image-generation:v1:2'
        }
      }
    } satisfies HostInvocationResult<ImageGenerationGetConfigurationOutput>
    invoke.mockResolvedValue(response)
    const bridge = createImageGenerationIpcBridge({
      invoke
    } as unknown as ImageGenerationIpcRenderer)
    const updateInput = {
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'image-generation:v1:1',
      adapterId: 'smartmlSeedream',
      endpointUrl: 'https://example.test/userapi/v1/images/generations',
      modelId: 'doubao-seedream-4-0-250828',
      capabilities: { textToImage: true, imageToImage: true },
      defaults: { sizePreset: '2K', watermark: true },
      credentialMutation: { type: 'replace', value: 'secret-that-must-remain-opaque' }
    } satisfies ImageGenerationUpdateConfigurationInput
    const setEnabledInput = {
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'image-generation:v1:2',
      enabled: true
    } satisfies ImageGenerationSetEnabledInput
    const artifactInput = {
      schemaVersion: 1,
      artifact: {
        artifactId: `sha256:${'a'.repeat(64)}`,
        uri: `image-artifact://sha256/${'a'.repeat(64)}`,
        kind: 'image',
        format: 'png',
        mimeType: 'image/png',
        width: 1,
        height: 1,
        sizeBytes: 1,
        sha256: 'a'.repeat(64)
      }
    } as const

    await expect(bridge.getConfiguration()).resolves.toBe(response)
    await expect(bridge.updateConfiguration(updateInput)).resolves.toBe(response)
    await expect(bridge.setEnabled(setEnabledInput)).resolves.toBe(response)
    await expect(bridge.getStatus()).resolves.toBe(response)
    await expect(bridge.readArtifact(artifactInput)).resolves.toBe(response)

    expect(invoke.mock.calls).toEqual([
      [IMAGE_GENERATION_GET_CONFIGURATION_CHANNEL],
      [IMAGE_GENERATION_UPDATE_CONFIGURATION_CHANNEL, updateInput],
      [IMAGE_GENERATION_SET_ENABLED_CHANNEL, setEnabledInput],
      [IMAGE_GENERATION_GET_STATUS_CHANNEL],
      [IMAGE_GENERATION_READ_ARTIFACT_CHANNEL, artifactInput]
    ])
  })

  it('rejects a legacy API key in a successful configuration projection', async () => {
    const invoke = vi.fn().mockResolvedValue({
      ok: true,
      value: {
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration: {
          adapterId: 'smartmlSeedream',
          endpointUrl: 'https://example.test/v1/images/generations',
          modelId: 'image-model',
          capabilities: { textToImage: true, imageToImage: true },
          defaults: { sizePreset: '2K', watermark: true },
          credentialStatus: 'configured',
          enabled: true,
          readiness: 'readyUnverified',
          revision: 'revision-1'
        },
        apiKey: 'must-not-cross-the-boundary'
      }
    })
    const bridge = createImageGenerationIpcBridge({
      invoke
    } as unknown as ImageGenerationIpcRenderer)

    await expect(bridge.getConfiguration()).rejects.toThrow(/unexpected field apiKey/)
  })
})
