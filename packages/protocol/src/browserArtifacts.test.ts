import { describe, expect, it } from 'vitest'

import {
  BROWSER_ARTIFACT_SCHEMA_VERSION,
  parseBrowserArtifactExportInput,
  parseBrowserArtifactExportOutput,
  parseBrowserArtifactReadInput,
  parseBrowserArtifactReadOutput,
  parseBrowserArtifactReference,
  parseBrowserArtifactToolProjection
} from './browserArtifacts'

const artifact = {
  schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
  artifactId: 'browser-artifact:123e4567-e89b-42d3-a456-426614174000',
  kind: 'image',
  displayName: 'page.png',
  mimeType: 'image/png',
  sizeBytes: 42,
  createdAt: 1_000,
  expiresAt: 2_000,
  lifecycle: 'run',
  owner: 'browser_automation',
  preview: 'image'
} as const

describe('Browser Artifact protocol', () => {
  it('parses only a bounded path-free Artifact reference', () => {
    expect(parseBrowserArtifactReference(artifact)).toEqual(artifact)
    expect(() =>
      parseBrowserArtifactReference({ ...artifact, managedPath: '/tmp/page.png' })
    ).toThrow(/unexpected field managedPath/)
    expect(() =>
      parseBrowserArtifactReference({ ...artifact, displayName: '../../page.png' })
    ).toThrow(/path-free/)
    expect(() =>
      parseBrowserArtifactReference({ ...artifact, displayName: '<img onerror=secret>.png' })
    ).toThrow(/path-free/)
    expect(() => parseBrowserArtifactReference({ ...artifact, displayName: ' page.png' })).toThrow(
      /canonical/
    )
    expect(() => parseBrowserArtifactReference({ ...artifact, mimeType: 'TEXT/PLAIN' })).toThrow(
      /canonical MIME/
    )
    expect(() => parseBrowserArtifactReference({ ...artifact, expiresAt: 1_000 })).toThrow(/expiry/)
  })

  it('requires the complete frozen reference when reading content', () => {
    const input = { schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION, artifact }
    expect(parseBrowserArtifactReadInput(input)).toEqual(input)
    expect(() =>
      parseBrowserArtifactReadInput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        artifactId: artifact.artifactId
      })
    ).toThrow(/unexpected field artifactId/)
    expect(
      parseBrowserArtifactReadOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        artifact: { ...artifact, sizeBytes: 2 },
        bytes: Uint8Array.from([1, 2])
      })
    ).toEqual({
      schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
      artifact: { ...artifact, sizeBytes: 2 },
      bytes: Uint8Array.from([1, 2])
    })
    expect(() =>
      parseBrowserArtifactReadOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        artifact: { ...artifact, kind: 'pdf', mimeType: 'application/pdf', preview: 'none' },
        bytes: Uint8Array.from({ length: 42 })
      })
    ).toThrow(/does not permit Renderer content/)
  })

  it('exports only an exact reference and returns no filesystem path', () => {
    const input = { schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION, artifact }
    expect(parseBrowserArtifactExportInput(input)).toEqual(input)
    expect(() => parseBrowserArtifactExportInput({ ...input, path: '/tmp/page.png' })).toThrow(
      /unexpected field path/
    )
    expect(
      parseBrowserArtifactExportOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        status: 'exported',
        displayName: 'saved-page.png'
      })
    ).toEqual({
      schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
      status: 'exported',
      displayName: 'saved-page.png'
    })
    expect(
      parseBrowserArtifactExportOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        status: 'cancelled'
      })
    ).toEqual({ schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION, status: 'cancelled' })
    expect(() =>
      parseBrowserArtifactExportOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        status: 'exported',
        displayName: 'page.png',
        path: '/tmp/page.png'
      })
    ).toThrow(/unexpected field path/)
    expect(() =>
      parseBrowserArtifactExportOutput({
        schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
        status: 'cancelled',
        displayName: 'page.png'
      })
    ).toThrow(/must be omitted/)
  })

  it('parses the narrow builtin tool projection without raw content', () => {
    const projection = {
      schemaVersion: BROWSER_ARTIFACT_SCHEMA_VERSION,
      type: 'builtin_capability_tool',
      contentOmitted: true,
      status: 'completed',
      artifacts: [artifact]
    } as const
    expect(parseBrowserArtifactToolProjection(projection)).toEqual(projection)
    expect(() =>
      parseBrowserArtifactToolProjection({ ...projection, rawResult: 'page secret' })
    ).toThrow(/unexpected field rawResult/)
    expect(() =>
      parseBrowserArtifactToolProjection({ ...projection, artifacts: [artifact, artifact] })
    ).toThrow(/duplicate/)
  })
})
