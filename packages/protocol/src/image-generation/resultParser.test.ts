import { describe, expect, it } from 'vitest'

import {
  AGENT_IMAGE_GENERATION_ARTIFACT_MAX_BYTES,
  AGENT_IMAGE_GENERATION_ARTIFACT_MAX_DIMENSION,
  AGENT_IMAGE_GENERATION_ARTIFACT_MAX_PIXELS,
  AGENT_IMAGE_GENERATION_FAILURE_TEXT_MAX_CHARACTERS,
  AGENT_IMAGE_GENERATION_MODEL_ID_MAX_BYTES,
  AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
  parseAgentImageGenerationResult
} from './index'

const digest = 'a'.repeat(64)

const audit = {
  executionId: `agent-v1:${'b'.repeat(64)}`,
  requestFingerprint: `sha256:${'c'.repeat(64)}`,
  providerProfileId: 'default',
  adapterId: 'smartmlSeedream',
  profileRevision: 7,
  modelId: 'doubao-seedream-4-0-250828',
  providerRequestId: `sha256:${'d'.repeat(64)}`,
  httpStatus: 200,
  createdAt: 1_000,
  completedAt: 1_125,
  durationMs: 125
} as const

const artifact = {
  artifactId: `sha256:${digest}`,
  uri: `image-artifact://sha256/${digest}`,
  kind: 'image',
  format: 'png',
  mimeType: 'image/png',
  width: 2_048,
  height: 2_048,
  sizeBytes: 4_096,
  sha256: digest
} as const

const failure = {
  code: 'providerFailed',
  phase: 'provider',
  message: 'The image provider rejected the request.',
  recovery: 'Review the request and try again.',
  retryable: false,
  generationMayHaveSucceeded: false,
  providerSucceeded: false,
  artifactCommitMayHaveSucceeded: false
} as const

const succeededResult = {
  schemaVersion: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
  status: 'succeeded',
  operation: 'generate',
  artifact,
  audit
} as const

const failedResult = {
  schemaVersion: AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION,
  status: 'failed',
  operation: 'edit',
  audit,
  failure
} as const

describe('Agent image generation result protocol', () => {
  it('accepts a successful result containing only a verified managed Artifact', () => {
    expect(parseAgentImageGenerationResult(succeededResult)).toEqual(succeededResult)
  })

  it.each([
    ['png', 'image/png'],
    ['jpeg', 'image/jpeg'],
    ['webp', 'image/webp']
  ] as const)('accepts the %s format only with its canonical MIME type', (format, mimeType) => {
    const result = {
      ...succeededResult,
      artifact: { ...artifact, format, mimeType }
    }
    expect(parseAgentImageGenerationResult(result)).toEqual(result)
  })

  it.each(['failed', 'cancelled', 'outcomeIndeterminate', 'commitIndeterminate'] as const)(
    'accepts a %s terminal result only with failure details',
    (status) => {
      const result = { ...failedResult, status }
      expect(parseAgentImageGenerationResult(result)).toEqual(result)
    }
  )

  it('enforces the success and non-success Artifact/failure invariant', () => {
    expect(() =>
      parseAgentImageGenerationResult({ ...succeededResult, artifact: undefined })
    ).toThrow(/must contain exactly one artifact/)
    expect(() => parseAgentImageGenerationResult({ ...succeededResult, failure })).toThrow(
      /must not contain failure/
    )
    expect(() => parseAgentImageGenerationResult({ ...failedResult, artifact })).toThrow(
      /must not contain artifact/
    )
    const withoutFailure = {
      schemaVersion: failedResult.schemaVersion,
      status: failedResult.status,
      operation: failedResult.operation,
      audit: failedResult.audit
    }
    expect(() => parseAgentImageGenerationResult(withoutFailure)).toThrow(/must contain failure/)
  })

  it('requires schema v1 and closed status, operation, format, code, and phase enums', () => {
    for (const value of [
      { ...succeededResult, schemaVersion: 2 },
      { ...succeededResult, status: 'running' },
      { ...succeededResult, operation: 'status' },
      { ...succeededResult, artifact: { ...artifact, format: 'gif' } },
      { ...failedResult, failure: { ...failure, code: 'unknown' } },
      { ...failedResult, failure: { ...failure, phase: 'download' } }
    ]) {
      expect(() => parseAgentImageGenerationResult(value)).toThrow()
    }
  })

  it('binds Artifact ID and capability URI to one canonical lower-case sha256', () => {
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, artifactId: `sha256:${'e'.repeat(64)}` }
      })
    ).toThrow(/artifactId.*sha256/)
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, uri: `image-artifact://sha256/${'e'.repeat(64)}` }
      })
    ).toThrow(/uri.*must equal/)
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, sha256: digest.toUpperCase() }
      })
    ).toThrow(/sha256.*canonical/)
  })

  it('rejects MIME/format mismatches', () => {
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, mimeType: 'image/jpeg' }
      })
    ).toThrow(/mimeType.*image\/png/)
  })

  it('bounds dimensions, pixel count, and encoded Artifact size', () => {
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, width: 0 }
      })
    ).toThrow(/width/)
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: { ...artifact, width: AGENT_IMAGE_GENERATION_ARTIFACT_MAX_DIMENSION + 1 }
      })
    ).toThrow(/width.*must not exceed/)
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: {
          ...artifact,
          width: 16_384,
          height: Math.floor(AGENT_IMAGE_GENERATION_ARTIFACT_MAX_PIXELS / 16_384) + 1
        }
      })
    ).toThrow(/pixel count/)
    expect(() =>
      parseAgentImageGenerationResult({
        ...succeededResult,
        artifact: {
          ...artifact,
          sizeBytes: AGENT_IMAGE_GENERATION_ARTIFACT_MAX_BYTES + 1
        }
      })
    ).toThrow(/sizeBytes.*must not exceed/)
  })

  it('bounds and validates every audit identity and timestamp', () => {
    for (const nextAudit of [
      { ...audit, executionId: ' '.repeat(3) },
      { ...audit, requestFingerprint: 'not-a-sha256' },
      { ...audit, adapterId: '-invalid' },
      { ...audit, modelId: 'x'.repeat(AGENT_IMAGE_GENERATION_MODEL_ID_MAX_BYTES + 1) },
      { ...audit, profileRevision: 0 },
      { ...audit, providerRequestId: 'provider-raw-id' },
      { ...audit, httpStatus: 600 },
      { ...audit, completedAt: audit.createdAt - 1 },
      { ...audit, durationMs: -1 }
    ]) {
      expect(() =>
        parseAgentImageGenerationResult({ ...succeededResult, audit: nextAudit })
      ).toThrow()
    }
  })

  it('bounds failure text and requires every uncertainty marker', () => {
    expect(() =>
      parseAgentImageGenerationResult({
        ...failedResult,
        failure: {
          ...failure,
          message: 'x'.repeat(AGENT_IMAGE_GENERATION_FAILURE_TEXT_MAX_CHARACTERS + 1)
        }
      })
    ).toThrow(/message.*at most/)
    const incompleteFailure = {
      code: failure.code,
      phase: failure.phase,
      message: failure.message,
      recovery: failure.recovery,
      retryable: failure.retryable,
      generationMayHaveSucceeded: failure.generationMayHaveSucceeded,
      artifactCommitMayHaveSucceeded: failure.artifactCommitMayHaveSucceeded
    }
    expect(() =>
      parseAgentImageGenerationResult({ ...failedResult, failure: incompleteFailure })
    ).toThrow(/providerSucceeded/)
  })

  it('rejects secret, endpoint, path, URL, and base64-shaped fields at every boundary', () => {
    for (const value of [
      { ...succeededResult, endpointUrl: 'https://provider.example/generate' },
      { ...succeededResult, apiKey: 'secret' },
      { ...succeededResult, inputBase64: 'iVBORw0KGgo=' },
      { ...succeededResult, artifact: { ...artifact, absolutePath: '/private/image.png' } },
      { ...succeededResult, artifact: { ...artifact, providerUrl: 'https://signed.example' } },
      { ...succeededResult, audit: { ...audit, credential: 'secret' } },
      { ...failedResult, failure: { ...failure, stagingPath: '/private/staging/image.png' } }
    ]) {
      expect(() => parseAgentImageGenerationResult(value)).toThrow(/unexpected field/)
    }
  })
})
