import { describe, expect, it } from 'vitest'

import {
  IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
  IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH,
  parseImageGenerationConfigurationErrorData,
  parseImageGenerationGetConfigurationOutput,
  parseImageGenerationStatus,
  parseImageGenerationUpdateConfigurationInput
} from './index'

const readyConfiguration = {
  adapterId: 'smartmlSeedream',
  endpointUrl: 'https://example.test/v1/images/generations',
  modelId: 'image-model',
  capabilities: { textToImage: true, imageToImage: true },
  defaults: { sizePreset: '2K', watermark: true },
  credentialStatus: 'configured',
  enabled: true,
  readiness: 'readyUnverified',
  revision: 'revision-1'
} as const

describe('image generation configuration protocol', () => {
  it('accepts a complete secret-free configuration snapshot', () => {
    expect(
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration: readyConfiguration
      })
    ).toEqual({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      configuration: readyConfiguration
    })
  })

  it('accepts the fail-closed default configuration returned when no record exists', () => {
    const configuration = {
      ...readyConfiguration,
      endpointUrl: '',
      modelId: '',
      credentialStatus: 'missing',
      enabled: false,
      readiness: 'disabled'
    } as const

    expect(
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration
      }).configuration
    ).toEqual(configuration)
  })

  it('keeps a credential-store outage recoverable without claiming the secret is configured', () => {
    const configuration = {
      ...readyConfiguration,
      credentialStatus: 'unavailable',
      readiness: 'credentialUnavailable'
    } as const

    expect(
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration
      }).configuration
    ).toEqual(configuration)
    expect(() =>
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration: { ...configuration, readiness: 'readyUnverified' }
      })
    ).toThrow(/readiness.*credentialUnavailable/)
  })

  it('requires text-to-image to remain the literal true', () => {
    expect(() =>
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration: {
          ...readyConfiguration,
          capabilities: { textToImage: false, imageToImage: true }
        }
      })
    ).toThrow(/textToImage.*literal true/)
  })

  it('rejects a readiness value inconsistent with the authoritative configuration', () => {
    expect(() =>
      parseImageGenerationGetConfigurationOutput({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        configuration: { ...readyConfiguration, endpointUrl: '', readiness: 'readyUnverified' }
      })
    ).toThrow(/readiness.*missingEndpoint/)
  })

  it('parses explicit credential mutations and never treats omission as keep', () => {
    const base = {
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'revision-1',
      adapterId: 'smartmlSeedream',
      endpointUrl: readyConfiguration.endpointUrl,
      modelId: readyConfiguration.modelId,
      capabilities: readyConfiguration.capabilities,
      defaults: readyConfiguration.defaults
    } as const

    expect(
      parseImageGenerationUpdateConfigurationInput({
        ...base,
        credentialMutation: { type: 'keep' }
      }).credentialMutation
    ).toEqual({ type: 'keep' })
    expect(
      parseImageGenerationUpdateConfigurationInput({
        ...base,
        credentialMutation: { type: 'replace', value: 'secret-token' }
      }).credentialMutation
    ).toEqual({ type: 'replace', value: 'secret-token' })
    expect(
      parseImageGenerationUpdateConfigurationInput({
        ...base,
        credentialMutation: { type: 'clear' }
      }).credentialMutation
    ).toEqual({ type: 'clear' })
    expect(() => parseImageGenerationUpdateConfigurationInput(base)).toThrow(/credentialMutation/)
  })

  it('bounds credentials and rejects whitespace-bearing bearer values', () => {
    const base = {
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'revision-1',
      adapterId: 'smartmlSeedream',
      endpointUrl: readyConfiguration.endpointUrl,
      modelId: readyConfiguration.modelId,
      capabilities: readyConfiguration.capabilities,
      defaults: readyConfiguration.defaults
    } as const

    expect(() =>
      parseImageGenerationUpdateConfigurationInput({
        ...base,
        credentialMutation: {
          type: 'replace',
          value: 'x'.repeat(IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH + 1)
        }
      })
    ).toThrow(/at most/)
    expect(() =>
      parseImageGenerationUpdateConfigurationInput({
        ...base,
        credentialMutation: { type: 'replace', value: 'secret token' }
      })
    ).toThrow(/must not contain whitespace/)
  })

  it('keeps status compact and validates its typed capabilities', () => {
    expect(
      parseImageGenerationStatus({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        adapterId: readyConfiguration.adapterId,
        configurationRevision: readyConfiguration.revision,
        enabled: true,
        readiness: 'readyUnverified',
        credentialStatus: 'configured',
        capabilities: readyConfiguration.capabilities,
        defaults: readyConfiguration.defaults
      })
    ).toEqual({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      adapterId: readyConfiguration.adapterId,
      configurationRevision: readyConfiguration.revision,
      enabled: true,
      readiness: 'readyUnverified',
      credentialStatus: 'configured',
      capabilities: readyConfiguration.capabilities,
      defaults: readyConfiguration.defaults
    })

    expect(() =>
      parseImageGenerationStatus({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        adapterId: readyConfiguration.adapterId,
        configurationRevision: readyConfiguration.revision,
        enabled: false,
        readiness: 'readyUnverified',
        credentialStatus: 'configured',
        capabilities: readyConfiguration.capabilities,
        defaults: readyConfiguration.defaults
      })
    ).toThrow(/disabled configuration must be disabled/)

    expect(
      parseImageGenerationStatus({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        adapterId: readyConfiguration.adapterId,
        configurationRevision: readyConfiguration.revision,
        enabled: true,
        readiness: 'credentialUnavailable',
        credentialStatus: 'unavailable',
        capabilities: readyConfiguration.capabilities,
        defaults: readyConfiguration.defaults
      })
    ).toMatchObject({
      readiness: 'credentialUnavailable',
      credentialStatus: 'unavailable'
    })
  })

  it('parses structured errors without accepting secret-shaped extra data', () => {
    expect(
      parseImageGenerationConfigurationErrorData({
        type: 'imageGenerationConfiguration',
        operation: 'updateConfiguration',
        code: 'revisionConflict',
        recovery: 'refreshConfiguration',
        message: 'The configuration changed.',
        currentRevision: 'revision-2'
      })
    ).toEqual({
      type: 'imageGenerationConfiguration',
      operation: 'updateConfiguration',
      code: 'revisionConflict',
      recovery: 'refreshConfiguration',
      message: 'The configuration changed.',
      currentRevision: 'revision-2'
    })
    expect(() =>
      parseImageGenerationConfigurationErrorData({
        type: 'imageGenerationConfiguration',
        operation: 'updateConfiguration',
        code: 'invalidCredential',
        recovery: 'reenterCredential',
        message: 'Invalid credential.',
        credential: 'must-not-cross-the-boundary'
      })
    ).toThrow(/unexpected field credential/)
  })
})
