import {
  IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
  type ImageGenerationCapabilities,
  type ImageGenerationConfiguration,
  type ImageGenerationConfigurationErrorData,
  type ImageGenerationCredentialMutation,
  type ImageGenerationDefaults,
  type ImageGenerationGetConfigurationOutput,
  type ImageGenerationSetEnabledInput,
  type ImageGenerationSetEnabledOutput,
  type ImageGenerationStatus,
  type ImageGenerationUpdateConfigurationInput,
  type ImageGenerationUpdateConfigurationOutput
} from './contracts'
import {
  expectBoolean,
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  invalidProtocolValue,
  optionalNonEmptyString
} from '../skills/validation'

export const IMAGE_GENERATION_ENDPOINT_URL_MAX_LENGTH = 2_048 as const
export const IMAGE_GENERATION_MODEL_ID_MAX_LENGTH = 512 as const
export const IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH = 8_192 as const
export const IMAGE_GENERATION_REVISION_MAX_LENGTH = 512 as const
export const IMAGE_GENERATION_ERROR_MESSAGE_MAX_LENGTH = 2_048 as const

export function parseImageGenerationGetConfigurationOutput(
  value: unknown
): ImageGenerationGetConfigurationOutput {
  const context = 'Image generation configuration response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'configuration'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    configuration: parseImageGenerationConfiguration(
      record.configuration,
      `${context}.configuration`
    )
  }
}

export function parseImageGenerationUpdateConfigurationInput(
  value: unknown
): ImageGenerationUpdateConfigurationInput {
  const context = 'Image generation configuration update request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'expectedRevision',
      'adapterId',
      'endpointUrl',
      'modelId',
      'capabilities',
      'defaults',
      'credentialMutation'
    ] as const,
    context
  )
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    expectedRevision: expectBoundedNonEmptyString(
      record.expectedRevision,
      IMAGE_GENERATION_REVISION_MAX_LENGTH,
      `${context}.expectedRevision`
    ),
    adapterId: expectAdapterId(record.adapterId, `${context}.adapterId`),
    endpointUrl: expectCanonicalBoundedString(
      record.endpointUrl,
      IMAGE_GENERATION_ENDPOINT_URL_MAX_LENGTH,
      `${context}.endpointUrl`
    ),
    modelId: expectCanonicalBoundedString(
      record.modelId,
      IMAGE_GENERATION_MODEL_ID_MAX_LENGTH,
      `${context}.modelId`
    ),
    capabilities: parseCapabilities(record.capabilities, `${context}.capabilities`),
    defaults: parseDefaults(record.defaults, `${context}.defaults`),
    credentialMutation: parseImageGenerationCredentialMutation(
      record.credentialMutation,
      `${context}.credentialMutation`
    )
  }
}

export function parseImageGenerationUpdateConfigurationOutput(
  value: unknown
): ImageGenerationUpdateConfigurationOutput {
  const context = 'Image generation configuration update response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'outcome', 'configuration'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    outcome: expectEnum(
      record.outcome,
      ['updated', 'alreadyCurrent'] as const,
      `${context}.outcome`
    ),
    configuration: parseImageGenerationConfiguration(
      record.configuration,
      `${context}.configuration`
    )
  }
}

export function parseImageGenerationSetEnabledInput(
  value: unknown
): ImageGenerationSetEnabledInput {
  const context = 'Image generation enablement request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'expectedRevision', 'enabled'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    expectedRevision: expectBoundedNonEmptyString(
      record.expectedRevision,
      IMAGE_GENERATION_REVISION_MAX_LENGTH,
      `${context}.expectedRevision`
    ),
    enabled: expectBoolean(record.enabled, `${context}.enabled`)
  }
}

export function parseImageGenerationSetEnabledOutput(
  value: unknown
): ImageGenerationSetEnabledOutput {
  const context = 'Image generation enablement response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'outcome', 'configuration'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    outcome: expectEnum(
      record.outcome,
      ['updated', 'alreadyCurrent'] as const,
      `${context}.outcome`
    ),
    configuration: parseImageGenerationConfiguration(
      record.configuration,
      `${context}.configuration`
    )
  }
}

export function parseImageGenerationStatus(value: unknown): ImageGenerationStatus {
  const context = 'Image generation status response'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'schemaVersion',
      'adapterId',
      'configurationRevision',
      'enabled',
      'readiness',
      'credentialStatus',
      'capabilities',
      'defaults'
    ] as const,
    context
  )
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  const status: ImageGenerationStatus = {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    adapterId: expectAdapterId(record.adapterId, `${context}.adapterId`),
    configurationRevision: expectBoundedNonEmptyString(
      record.configurationRevision,
      IMAGE_GENERATION_REVISION_MAX_LENGTH,
      `${context}.configurationRevision`
    ),
    enabled: expectBoolean(record.enabled, `${context}.enabled`),
    readiness: expectReadiness(record.readiness, `${context}.readiness`),
    credentialStatus: expectEnum(
      record.credentialStatus,
      ['missing', 'configured', 'unavailable'] as const,
      `${context}.credentialStatus`
    ),
    capabilities: parseCapabilities(record.capabilities, `${context}.capabilities`),
    defaults: parseDefaults(record.defaults, `${context}.defaults`)
  }
  validateStatusReadiness(status, context)
  return status
}

export function parseImageGenerationConfigurationErrorData(
  value: unknown
): ImageGenerationConfigurationErrorData {
  const context = 'Image generation configuration error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'type',
      'operation',
      'code',
      'recovery',
      'message',
      'currentRevision',
      'retryAfterMs',
      'configurationMayHaveChanged'
    ] as const,
    context
  )
  if (record.type !== 'imageGenerationConfiguration') {
    throw invalidProtocolValue(context, 'type must be imageGenerationConfiguration')
  }
  const currentRevision = optionalBoundedNonEmptyString(
    record.currentRevision,
    IMAGE_GENERATION_REVISION_MAX_LENGTH,
    `${context}.currentRevision`
  )
  const retryAfterMs =
    record.retryAfterMs === undefined
      ? undefined
      : expectSafeInteger(record.retryAfterMs, `${context}.retryAfterMs`, 0)
  if (
    record.configurationMayHaveChanged !== undefined &&
    record.configurationMayHaveChanged !== true
  ) {
    throw invalidProtocolValue(
      `${context}.configurationMayHaveChanged`,
      'expected the literal true'
    )
  }
  const message = expectBoundedNonEmptyString(
    record.message,
    IMAGE_GENERATION_ERROR_MESSAGE_MAX_LENGTH,
    `${context}.message`
  )
  return {
    type: 'imageGenerationConfiguration',
    operation: expectEnum(
      record.operation,
      ['getConfiguration', 'updateConfiguration', 'setEnabled', 'getStatus'] as const,
      `${context}.operation`
    ),
    code: expectEnum(
      record.code,
      [
        'invalidRequest',
        'revisionConflict',
        'unsupportedAdapter',
        'missingEndpoint',
        'invalidEndpoint',
        'insecureEndpoint',
        'missingModel',
        'invalidModelId',
        'missingCredential',
        'invalidCredential',
        'credentialReplacementRequired',
        'storageUnavailable',
        'credentialStoreUnavailable',
        'commitIndeterminate',
        'unavailable'
      ] as const,
      `${context}.code`
    ),
    recovery: expectEnum(
      record.recovery,
      [
        'fixConfiguration',
        'refreshConfiguration',
        'reenterCredential',
        'retry',
        'contactSupport'
      ] as const,
      `${context}.recovery`
    ),
    message,
    ...(currentRevision === undefined ? {} : { currentRevision }),
    ...(retryAfterMs === undefined ? {} : { retryAfterMs }),
    ...(record.configurationMayHaveChanged === true
      ? { configurationMayHaveChanged: true as const }
      : {})
  }
}

function parseImageGenerationConfiguration(
  value: unknown,
  context: string
): ImageGenerationConfiguration {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'adapterId',
      'endpointUrl',
      'modelId',
      'capabilities',
      'defaults',
      'credentialStatus',
      'enabled',
      'readiness',
      'revision'
    ] as const,
    context
  )
  const configuration: ImageGenerationConfiguration = {
    adapterId: expectAdapterId(record.adapterId, `${context}.adapterId`),
    endpointUrl: expectCanonicalBoundedString(
      record.endpointUrl,
      IMAGE_GENERATION_ENDPOINT_URL_MAX_LENGTH,
      `${context}.endpointUrl`
    ),
    modelId: expectCanonicalBoundedString(
      record.modelId,
      IMAGE_GENERATION_MODEL_ID_MAX_LENGTH,
      `${context}.modelId`
    ),
    capabilities: parseCapabilities(record.capabilities, `${context}.capabilities`),
    defaults: parseDefaults(record.defaults, `${context}.defaults`),
    credentialStatus: expectEnum(
      record.credentialStatus,
      ['missing', 'configured', 'unavailable'] as const,
      `${context}.credentialStatus`
    ),
    enabled: expectBoolean(record.enabled, `${context}.enabled`),
    readiness: expectReadiness(record.readiness, `${context}.readiness`),
    revision: expectBoundedNonEmptyString(
      record.revision,
      IMAGE_GENERATION_REVISION_MAX_LENGTH,
      `${context}.revision`
    )
  }
  const expectedReadiness = deriveConfigurationReadiness(configuration)
  if (configuration.readiness !== expectedReadiness) {
    throw invalidProtocolValue(
      `${context}.readiness`,
      `expected ${expectedReadiness} for this configuration snapshot`
    )
  }
  return configuration
}

function parseCapabilities(value: unknown, context: string): ImageGenerationCapabilities {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['textToImage', 'imageToImage'] as const, context)
  if (record.textToImage !== true) {
    throw invalidProtocolValue(`${context}.textToImage`, 'expected the literal true')
  }
  return {
    textToImage: true,
    imageToImage: expectBoolean(record.imageToImage, `${context}.imageToImage`)
  }
}

function parseDefaults(value: unknown, context: string): ImageGenerationDefaults {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['sizePreset', 'watermark'] as const, context)
  return {
    sizePreset: expectEnum(record.sizePreset, ['2K'] as const, `${context}.sizePreset`),
    watermark: expectBoolean(record.watermark, `${context}.watermark`)
  }
}

function parseImageGenerationCredentialMutation(
  value: unknown,
  context: string
): ImageGenerationCredentialMutation {
  const record = expectRecord(value, context)
  switch (record.type) {
    case 'keep':
      expectOnlyKeys(record, ['type'] as const, context)
      return { type: 'keep' }
    case 'clear':
      expectOnlyKeys(record, ['type'] as const, context)
      return { type: 'clear' }
    case 'replace': {
      expectOnlyKeys(record, ['type', 'value'] as const, context)
      const credential = expectBoundedNonEmptyString(
        record.value,
        IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH,
        `${context}.value`
      )
      if (credential !== credential.trim()) {
        throw invalidProtocolValue(`${context}.value`, 'must not contain surrounding whitespace')
      }
      if (/\s/u.test(credential)) {
        throw invalidProtocolValue(`${context}.value`, 'must not contain whitespace')
      }
      return { type: 'replace', value: credential }
    }
    default:
      throw invalidProtocolValue(context, `unknown type ${String(record.type)}`)
  }
}

function expectAdapterId(value: unknown, context: string): 'smartmlSeedream' {
  return expectEnum(value, ['smartmlSeedream'] as const, context)
}

function expectReadiness(
  value: unknown,
  context: string
): ImageGenerationConfiguration['readiness'] {
  return expectEnum(
    value,
    [
      'disabled',
      'missingEndpoint',
      'missingModel',
      'missingCredential',
      'credentialUnavailable',
      'readyUnverified'
    ] as const,
    context
  )
}

function expectCanonicalBoundedString(value: unknown, maximum: number, context: string): string {
  const result = expectString(value, context)
  if (result.length > maximum) {
    throw invalidProtocolValue(context, `must contain at most ${maximum} characters`)
  }
  if (result.includes('\0')) {
    throw invalidProtocolValue(context, 'must not contain NUL')
  }
  if (result !== result.trim()) {
    throw invalidProtocolValue(context, 'must not contain surrounding whitespace')
  }
  return result
}

function expectBoundedNonEmptyString(value: unknown, maximum: number, context: string): string {
  const result = expectNonEmptyString(value, context)
  if (result.length > maximum) {
    throw invalidProtocolValue(context, `must contain at most ${maximum} characters`)
  }
  return result
}

function optionalBoundedNonEmptyString(
  value: unknown,
  maximum: number,
  context: string
): string | undefined {
  const result = optionalNonEmptyString(value, context)
  if (result !== undefined && result.length > maximum) {
    throw invalidProtocolValue(context, `must contain at most ${maximum} characters`)
  }
  return result
}

function deriveConfigurationReadiness(
  configuration: Pick<
    ImageGenerationConfiguration,
    'enabled' | 'endpointUrl' | 'modelId' | 'credentialStatus'
  >
): ImageGenerationConfiguration['readiness'] {
  if (!configuration.enabled) return 'disabled'
  if (configuration.endpointUrl.length === 0) return 'missingEndpoint'
  if (configuration.modelId.length === 0) return 'missingModel'
  if (configuration.credentialStatus === 'missing') return 'missingCredential'
  if (configuration.credentialStatus === 'unavailable') return 'credentialUnavailable'
  return 'readyUnverified'
}

function validateStatusReadiness(status: ImageGenerationStatus, context: string): void {
  if (!status.enabled && status.readiness !== 'disabled') {
    throw invalidProtocolValue(`${context}.readiness`, 'disabled configuration must be disabled')
  }
  if (status.enabled && status.readiness === 'disabled') {
    throw invalidProtocolValue(`${context}.readiness`, 'enabled configuration cannot be disabled')
  }
  if (status.readiness === 'missingCredential' && status.credentialStatus !== 'missing') {
    throw invalidProtocolValue(
      `${context}.readiness`,
      'missingCredential requires a missing credential status'
    )
  }
  if (status.readiness === 'credentialUnavailable' && status.credentialStatus !== 'unavailable') {
    throw invalidProtocolValue(
      `${context}.readiness`,
      'credentialUnavailable requires an unavailable credential status'
    )
  }
  if (status.readiness === 'readyUnverified' && status.credentialStatus !== 'configured') {
    throw invalidProtocolValue(
      `${context}.readiness`,
      'readyUnverified requires a configured credential status'
    )
  }
}
