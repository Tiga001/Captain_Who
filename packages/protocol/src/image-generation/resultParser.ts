import {
  AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
  type AgentImageGenerationArtifact,
  type AgentImageGenerationArtifactFormat,
  type AgentImageGenerationAudit,
  type AgentImageGenerationFailure,
  type AgentImageGenerationResult
} from './contracts'
import {
  expectBoolean,
  expectEnum,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectSchemaVersion,
  invalidProtocolValue
} from '../skills/validation'

export const AGENT_IMAGE_GENERATION_ARTIFACT_MAX_BYTES = 128 * 1024 * 1024
export const AGENT_IMAGE_GENERATION_ARTIFACT_MAX_DIMENSION = 16_384
export const AGENT_IMAGE_GENERATION_ARTIFACT_MAX_PIXELS = 64 * 1024 * 1024

export const AGENT_IMAGE_GENERATION_EXECUTION_ID_MAX_BYTES = 256
export const AGENT_IMAGE_GENERATION_PROFILE_ID_MAX_BYTES = 256
export const AGENT_IMAGE_GENERATION_ADAPTER_ID_MAX_BYTES = 128
export const AGENT_IMAGE_GENERATION_MODEL_ID_MAX_BYTES = 512
export const AGENT_IMAGE_GENERATION_PROVIDER_REQUEST_ID_MAX_BYTES = 256
export const AGENT_IMAGE_GENERATION_FAILURE_TEXT_MAX_CHARACTERS = 512

const SHA256_PATTERN = /^[0-9a-f]{64}$/u
const SHA256_IDENTIFIER_PATTERN = /^sha256:[0-9a-f]{64}$/u
const OPAQUE_PROVIDER_REQUEST_ID_PATTERN = /^sha256:(?:[0-9a-f]{32}|[0-9a-f]{64})$/u
const ADAPTER_ID_PATTERN = /^[A-Za-z][A-Za-z0-9._-]*$/u

/**
 * Parses the public v1 Agent image-generation result and rejects every field outside that
 * contract. In particular, provider URLs, credentials, local paths, and inline image bytes can
 * never be smuggled through an otherwise valid result.
 */
export function parseAgentImageGenerationResult(value: unknown): AgentImageGenerationResult {
  const context = 'Agent image generation result'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    ['schemaVersion', 'status', 'operation', 'artifact', 'audit', 'failure'] as const,
    context
  )
  expectSchemaVersion(record, AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION, context)

  const status = expectEnum(
    record.status,
    ['succeeded', 'failed', 'cancelled', 'outcomeIndeterminate', 'commitIndeterminate'] as const,
    `${context}.status`
  )
  const operation = expectEnum(
    record.operation,
    ['generate', 'edit'] as const,
    `${context}.operation`
  )
  const audit = parseAudit(record.audit, `${context}.audit`)
  const hasArtifact = Object.prototype.hasOwnProperty.call(record, 'artifact')
  const hasFailure = Object.prototype.hasOwnProperty.call(record, 'failure')

  if (status === 'succeeded') {
    if (!hasArtifact || record.artifact === undefined) {
      throw invalidProtocolValue(context, 'succeeded result must contain exactly one artifact')
    }
    if (hasFailure) {
      throw invalidProtocolValue(context, 'succeeded result must not contain failure')
    }
    return {
      schemaVersion: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
      status,
      operation,
      artifact: parseArtifact(record.artifact, `${context}.artifact`),
      audit
    }
  }

  if (hasArtifact) {
    throw invalidProtocolValue(context, `${status} result must not contain artifact`)
  }
  if (!hasFailure || record.failure === undefined) {
    throw invalidProtocolValue(context, `${status} result must contain failure`)
  }
  return {
    schemaVersion: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
    status,
    operation,
    audit,
    failure: parseFailure(record.failure, `${context}.failure`)
  }
}

function parseArtifact(value: unknown, context: string): AgentImageGenerationArtifact {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'artifactId',
      'uri',
      'kind',
      'format',
      'mimeType',
      'width',
      'height',
      'sizeBytes',
      'sha256'
    ] as const,
    context
  )

  const sha256 = expectPatternString(record.sha256, SHA256_PATTERN, `${context}.sha256`)
  const artifactId = expectPatternString(
    record.artifactId,
    SHA256_IDENTIFIER_PATTERN,
    `${context}.artifactId`
  )
  if (artifactId !== `sha256:${sha256}`) {
    throw invalidProtocolValue(`${context}.artifactId`, 'must identify the artifact sha256')
  }
  const uri = expectExactString(record.uri, `image-artifact://sha256/${sha256}`, `${context}.uri`)
  const format = expectEnum(record.format, ['png', 'jpeg', 'webp'] as const, `${context}.format`)
  const mimeType = expectExactString(
    record.mimeType,
    mimeTypeForFormat(format),
    `${context}.mimeType`
  )
  const width = expectBoundedPositiveInteger(
    record.width,
    AGENT_IMAGE_GENERATION_ARTIFACT_MAX_DIMENSION,
    `${context}.width`
  )
  const height = expectBoundedPositiveInteger(
    record.height,
    AGENT_IMAGE_GENERATION_ARTIFACT_MAX_DIMENSION,
    `${context}.height`
  )
  if (width * height > AGENT_IMAGE_GENERATION_ARTIFACT_MAX_PIXELS) {
    throw invalidProtocolValue(
      context,
      `pixel count must not exceed ${AGENT_IMAGE_GENERATION_ARTIFACT_MAX_PIXELS}`
    )
  }
  const sizeBytes = expectBoundedPositiveInteger(
    record.sizeBytes,
    AGENT_IMAGE_GENERATION_ARTIFACT_MAX_BYTES,
    `${context}.sizeBytes`
  )

  return {
    artifactId,
    uri,
    kind: expectEnum(record.kind, ['image'] as const, `${context}.kind`),
    format,
    mimeType,
    width,
    height,
    sizeBytes,
    sha256
  }
}

function parseAudit(value: unknown, context: string): AgentImageGenerationAudit {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'executionId',
      'requestFingerprint',
      'providerProfileId',
      'adapterId',
      'profileRevision',
      'modelId',
      'providerRequestId',
      'httpStatus',
      'createdAt',
      'completedAt',
      'durationMs'
    ] as const,
    context
  )

  const executionId = expectCanonicalBoundedString(
    record.executionId,
    AGENT_IMAGE_GENERATION_EXECUTION_ID_MAX_BYTES,
    `${context}.executionId`
  )
  const requestFingerprint = expectPatternString(
    record.requestFingerprint,
    SHA256_IDENTIFIER_PATTERN,
    `${context}.requestFingerprint`
  )
  const providerProfileId = expectCanonicalBoundedString(
    record.providerProfileId,
    AGENT_IMAGE_GENERATION_PROFILE_ID_MAX_BYTES,
    `${context}.providerProfileId`
  )
  const adapterId = expectCanonicalBoundedString(
    record.adapterId,
    AGENT_IMAGE_GENERATION_ADAPTER_ID_MAX_BYTES,
    `${context}.adapterId`
  )
  if (!ADAPTER_ID_PATTERN.test(adapterId)) {
    throw invalidProtocolValue(`${context}.adapterId`, 'has an invalid stable identifier')
  }
  const modelId = expectCanonicalBoundedString(
    record.modelId,
    AGENT_IMAGE_GENERATION_MODEL_ID_MAX_BYTES,
    `${context}.modelId`
  )
  const profileRevision = expectSafeInteger(record.profileRevision, `${context}.profileRevision`, 1)
  const createdAt = expectSafeInteger(record.createdAt, `${context}.createdAt`, 0)
  const completedAt = expectSafeInteger(record.completedAt, `${context}.completedAt`, 0)
  if (completedAt < createdAt) {
    throw invalidProtocolValue(`${context}.completedAt`, 'must not precede createdAt')
  }
  const durationMs = expectSafeInteger(record.durationMs, `${context}.durationMs`, 0)

  const hasProviderRequestId = Object.prototype.hasOwnProperty.call(record, 'providerRequestId')
  const providerRequestId = hasProviderRequestId
    ? expectCanonicalBoundedString(
        record.providerRequestId,
        AGENT_IMAGE_GENERATION_PROVIDER_REQUEST_ID_MAX_BYTES,
        `${context}.providerRequestId`
      )
    : undefined
  if (
    providerRequestId !== undefined &&
    !OPAQUE_PROVIDER_REQUEST_ID_PATTERN.test(providerRequestId)
  ) {
    throw invalidProtocolValue(
      `${context}.providerRequestId`,
      'must be an opaque sha256 request identifier'
    )
  }

  const hasHttpStatus = Object.prototype.hasOwnProperty.call(record, 'httpStatus')
  const httpStatus = hasHttpStatus
    ? expectSafeInteger(record.httpStatus, `${context}.httpStatus`, 100)
    : undefined
  if (httpStatus !== undefined && httpStatus > 599) {
    throw invalidProtocolValue(`${context}.httpStatus`, 'must be between 100 and 599')
  }

  return {
    executionId,
    requestFingerprint,
    providerProfileId,
    adapterId,
    profileRevision,
    modelId,
    ...(providerRequestId === undefined ? {} : { providerRequestId }),
    ...(httpStatus === undefined ? {} : { httpStatus }),
    createdAt,
    completedAt,
    durationMs
  }
}

function parseFailure(value: unknown, context: string): AgentImageGenerationFailure {
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'code',
      'phase',
      'message',
      'recovery',
      'retryable',
      'generationMayHaveSucceeded',
      'providerSucceeded',
      'artifactCommitMayHaveSucceeded'
    ] as const,
    context
  )
  return {
    code: expectEnum(
      record.code,
      [
        'providerFailed',
        'cancelled',
        'deadlineExceeded',
        'artifactFailed',
        'commitIndeterminate',
        'executionInterrupted',
        'journalUnavailable'
      ] as const,
      `${context}.code`
    ),
    phase: expectEnum(
      record.phase,
      [
        'admission',
        'configuration',
        'provider',
        'artifactDownload',
        'artifactPublish',
        'journal',
        'recovery'
      ] as const,
      `${context}.phase`
    ),
    message: expectBoundedText(
      record.message,
      AGENT_IMAGE_GENERATION_FAILURE_TEXT_MAX_CHARACTERS,
      `${context}.message`
    ),
    recovery: expectBoundedText(
      record.recovery,
      AGENT_IMAGE_GENERATION_FAILURE_TEXT_MAX_CHARACTERS,
      `${context}.recovery`
    ),
    retryable: expectBoolean(record.retryable, `${context}.retryable`),
    generationMayHaveSucceeded: expectBoolean(
      record.generationMayHaveSucceeded,
      `${context}.generationMayHaveSucceeded`
    ),
    providerSucceeded: expectBoolean(record.providerSucceeded, `${context}.providerSucceeded`),
    artifactCommitMayHaveSucceeded: expectBoolean(
      record.artifactCommitMayHaveSucceeded,
      `${context}.artifactCommitMayHaveSucceeded`
    )
  }
}

function mimeTypeForFormat(format: AgentImageGenerationArtifactFormat): string {
  switch (format) {
    case 'png':
      return 'image/png'
    case 'jpeg':
      return 'image/jpeg'
    case 'webp':
      return 'image/webp'
  }
}

function expectBoundedPositiveInteger(value: unknown, maximum: number, context: string): number {
  const result = expectSafeInteger(value, context, 1)
  if (result > maximum) {
    throw invalidProtocolValue(context, `must not exceed ${maximum}`)
  }
  return result
}

function expectCanonicalBoundedString(
  value: unknown,
  maximumBytes: number,
  context: string
): string {
  if (typeof value !== 'string' || value.length === 0) {
    throw invalidProtocolValue(context, 'expected a non-empty string')
  }
  if (value !== value.trim()) {
    throw invalidProtocolValue(context, 'must not contain surrounding whitespace')
  }
  if (containsControlCharacter(value)) {
    throw invalidProtocolValue(context, 'must not contain control characters')
  }
  if (new TextEncoder().encode(value).length > maximumBytes) {
    throw invalidProtocolValue(context, `must contain at most ${maximumBytes} UTF-8 bytes`)
  }
  return value
}

function expectBoundedText(value: unknown, maximumCharacters: number, context: string): string {
  if (typeof value !== 'string' || value.trim().length === 0) {
    throw invalidProtocolValue(context, 'expected a non-empty string')
  }
  if (containsControlCharacter(value)) {
    throw invalidProtocolValue(context, 'must not contain control characters')
  }
  if ([...value].length > maximumCharacters) {
    throw invalidProtocolValue(context, `must contain at most ${maximumCharacters} characters`)
  }
  return value
}

function expectPatternString(value: unknown, pattern: RegExp, context: string): string {
  if (typeof value !== 'string' || !pattern.test(value)) {
    throw invalidProtocolValue(context, 'has an invalid canonical value')
  }
  return value
}

function expectExactString(value: unknown, expected: string, context: string): string {
  if (value !== expected) {
    throw invalidProtocolValue(context, `must equal ${expected}`)
  }
  return expected
}

function containsControlCharacter(value: string): boolean {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0)
    return (
      codePoint !== undefined && (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f))
    )
  })
}
