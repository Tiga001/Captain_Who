import {
  IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
  IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
  type ImageGenerationArtifactErrorData,
  type ImageGenerationArtifactReadInput,
  type ImageGenerationArtifactReadOutput,
  type ManagedArtifactReadIdentity,
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
import { parseAgentImageGenerationArtifact } from './resultParser'
import {
  expectBoolean,
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  expectString,
  hasAsciiControlCharacter,
  invalidProtocolValue,
  optionalNonEmptyString
} from '../skills/validation'

export const IMAGE_GENERATION_ENDPOINT_URL_MAX_LENGTH = 2_048 as const
export const IMAGE_GENERATION_MODEL_ID_MAX_LENGTH = 512 as const
export const IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH = 8_192 as const
export const IMAGE_GENERATION_REVISION_MAX_LENGTH = 512 as const
export const IMAGE_GENERATION_ERROR_MESSAGE_MAX_LENGTH = 2_048 as const
export const IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES = 128 * 1024 * 1024
const IMAGE_GENERATION_IMAGE_ARTIFACT_READ_MAX_BYTES = 32 * 1024 * 1024
const BASE64_PAYLOAD_PATTERN = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u
const SHA256_PATTERN = /^[0-9a-f]{64}$/u

function parseManagedArtifactReadIdentity(
  value: unknown,
  context: string
): ManagedArtifactReadIdentity {
  const record = expectRecord(value, context)
  if (record.kind === 'image') return parseAgentImageGenerationArtifact(value, context)
  expectOnlyKeys(
    record,
    ['artifactId', 'uri', 'kind', 'format', 'mimeType', 'sizeBytes', 'sha256'] as const,
    context
  )
  if (
    record.kind !== 'document' ||
    record.format !== 'pdf' ||
    record.mimeType !== 'application/pdf'
  ) {
    throw invalidProtocolValue(context, 'must be a managed PDF document identity')
  }
  const sha256 = expectString(record.sha256, `${context}.sha256`)
  const artifactId = expectString(record.artifactId, `${context}.artifactId`)
  const uri = expectString(record.uri, `${context}.uri`)
  const sizeBytes = expectSafeInteger(record.sizeBytes, `${context}.sizeBytes`, 1)
  if (
    !SHA256_PATTERN.test(sha256) ||
    artifactId !== `sha256:${sha256}` ||
    uri !== `artifact://sha256/${sha256}` ||
    sizeBytes <= 0
  ) {
    throw invalidProtocolValue(context, 'contains a mismatched immutable document identity')
  }
  return {
    artifactId,
    uri,
    kind: 'document',
    format: 'pdf',
    mimeType: 'application/pdf',
    sizeBytes,
    sha256
  }
}

export function parseImageGenerationArtifactReadInput(
  value: unknown
): ImageGenerationArtifactReadInput {
  const context = 'Image generation Artifact read request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'artifact', 'conversationId', 'observerRootConversationId'] as const,
    context
  )
  expectSchemaVersion(record, IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION, context)
  const artifact = parseManagedArtifactReadIdentity(record.artifact, `${context}.artifact`)
  const conversationId = optionalNonEmptyString(record.conversationId, `${context}.conversationId`)
  const observerRootConversationId = optionalNonEmptyString(
    record.observerRootConversationId,
    `${context}.observerRootConversationId`
  )
  if (conversationId && (conversationId.length > 512 || hasAsciiControlCharacter(conversationId))) {
    throw invalidProtocolValue(`${context}.conversationId`, 'must be a bounded safe identifier')
  }
  if (
    observerRootConversationId &&
    (observerRootConversationId.length > 512 ||
      hasAsciiControlCharacter(observerRootConversationId))
  ) {
    throw invalidProtocolValue(
      `${context}.observerRootConversationId`,
      'must be a bounded safe identifier'
    )
  }
  if (observerRootConversationId && !conversationId) {
    throw invalidProtocolValue(
      `${context}.observerRootConversationId`,
      'requires an exact child conversationId'
    )
  }
  const maximumBytes =
    artifact.kind === 'image'
      ? IMAGE_GENERATION_IMAGE_ARTIFACT_READ_MAX_BYTES
      : IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES
  if (artifact.sizeBytes > maximumBytes) {
    throw invalidProtocolValue(`${context}.artifact.sizeBytes`, `must not exceed ${maximumBytes}`)
  }
  return {
    schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
    artifact,
    ...(conversationId ? { conversationId } : {}),
    ...(observerRootConversationId ? { observerRootConversationId } : {})
  }
}

export function parseImageGenerationArtifactReadOutput(
  value: unknown
): ImageGenerationArtifactReadOutput {
  const context = 'Image generation Artifact read response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'artifact', 'fileName', 'dataBase64'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION, context)
  const artifact = parseManagedArtifactReadIdentity(record.artifact, `${context}.artifact`)
  const maximumBytes =
    artifact.kind === 'image'
      ? IMAGE_GENERATION_IMAGE_ARTIFACT_READ_MAX_BYTES
      : IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES
  if (artifact.sizeBytes > maximumBytes) {
    throw invalidProtocolValue(`${context}.artifact.sizeBytes`, `must not exceed ${maximumBytes}`)
  }
  const expectedFileName =
    artifact.kind === 'image'
      ? `generated-image-${artifact.sha256.slice(0, 12)}.${extensionForArtifactFormat(artifact.format)}`
      : `artifact-${artifact.sha256.slice(0, 12)}.pdf`
  const fileName = expectString(record.fileName, `${context}.fileName`)
  if (fileName !== expectedFileName) {
    throw invalidProtocolValue(`${context}.fileName`, 'does not match the immutable Artifact')
  }
  const dataBase64 = expectString(record.dataBase64, `${context}.dataBase64`)
  const expectedEncodedLength = Math.ceil(artifact.sizeBytes / 3) * 4
  if (dataBase64.length !== expectedEncodedLength || !BASE64_PAYLOAD_PATTERN.test(dataBase64)) {
    throw invalidProtocolValue(
      `${context}.dataBase64`,
      'must be canonical base64 with the declared byte length'
    )
  }
  return {
    schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
    artifact,
    fileName,
    dataBase64
  }
}

export function parseImageGenerationArtifactErrorData(
  value: unknown
): ImageGenerationArtifactErrorData {
  const context = 'Image generation Artifact error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['type', 'operation', 'code', 'recovery', 'message', 'retryable'] as const,
    context
  )
  if (record.type !== 'imageGenerationArtifact') {
    throw invalidProtocolValue(context, 'type must be imageGenerationArtifact')
  }
  return {
    type: 'imageGenerationArtifact',
    operation: expectEnum(record.operation, ['read'] as const, `${context}.operation`),
    code: expectEnum(
      record.code,
      ['invalidRequest', 'notFound', 'integrityCheckFailed', 'tooLarge', 'unavailable'] as const,
      `${context}.code`
    ),
    recovery: expectEnum(
      record.recovery,
      ['doNotRetry', 'retry', 'regenerate'] as const,
      `${context}.recovery`
    ),
    message: expectBoundedNonEmptyString(
      record.message,
      IMAGE_GENERATION_ERROR_MESSAGE_MAX_LENGTH,
      `${context}.message`
    ),
    retryable: expectBoolean(record.retryable, `${context}.retryable`)
  }
}

export function parseImageGenerationGetConfigurationOutput(
  value: unknown
): ImageGenerationGetConfigurationOutput {
  const context = 'Image generation configuration response'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['schemaVersion', 'configuration', 'apiKey'] as const, context)
  expectSchemaVersion(record, IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION, context)
  const configuration = parseImageGenerationConfiguration(
    record.configuration,
    `${context}.configuration`
  )
  const apiKey =
    record.apiKey === null
      ? null
      : parseImageGenerationCredentialValue(record.apiKey, `${context}.apiKey`)
  if ((configuration.credentialStatus === 'configured') !== (apiKey !== null)) {
    throw invalidProtocolValue(
      `${context}.apiKey`,
      'must be present exactly when credentialStatus is configured'
    )
  }
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    configuration,
    apiKey
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
      const credential = parseImageGenerationCredentialValue(record.value, `${context}.value`)
      return { type: 'replace', value: credential }
    }
    default:
      throw invalidProtocolValue(context, `unknown type ${String(record.type)}`)
  }
}

function parseImageGenerationCredentialValue(value: unknown, context: string): string {
  const credential = expectBoundedNonEmptyString(
    value,
    IMAGE_GENERATION_CREDENTIAL_MAX_LENGTH,
    context
  )
  if (credential !== credential.trim()) {
    throw invalidProtocolValue(context, 'must not contain surrounding whitespace')
  }
  if (/\s/u.test(credential)) {
    throw invalidProtocolValue(context, 'must not contain whitespace')
  }
  return credential
}

function expectAdapterId(value: unknown, context: string): 'smartmlSeedream' {
  return expectEnum(value, ['smartmlSeedream'] as const, context)
}

function extensionForArtifactFormat(format: 'png' | 'jpeg' | 'webp'): 'png' | 'jpg' | 'webp' {
  if (format === 'jpeg') return 'jpg'
  return format
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
