import { describe, expect, it } from 'vitest'

import {
  IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
  IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES,
  parseImageGenerationArtifactErrorData,
  parseImageGenerationArtifactReadInput,
  parseImageGenerationArtifactReadOutput
} from './index'

const sha256 = '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824'
const artifact = {
  artifactId: `sha256:${sha256}`,
  uri: `image-artifact://sha256/${sha256}`,
  kind: 'image',
  format: 'png',
  mimeType: 'image/png',
  width: 1,
  height: 1,
  sizeBytes: 5,
  sha256
} as const

describe('image generation Artifact read protocol', () => {
  it('accepts only a complete frozen Artifact identity', () => {
    const input = {
      schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
      artifact
    } as const
    expect(parseImageGenerationArtifactReadInput(input)).toEqual(input)
    expect(
      parseImageGenerationArtifactReadInput({ ...input, conversationId: 'conversation-1' })
    ).toEqual({ ...input, conversationId: 'conversation-1' })
    expect(() =>
      parseImageGenerationArtifactReadInput({ ...input, conversationId: 'bad\nconversation' })
    ).toThrow(/conversationId/)
    expect(() =>
      parseImageGenerationArtifactReadInput({
        ...input,
        artifact: { ...artifact, providerUrl: 'https://provider.test/private' }
      })
    ).toThrow(/unexpected field providerUrl/)
    expect(() =>
      parseImageGenerationArtifactReadInput({
        ...input,
        artifact: {
          ...artifact,
          sizeBytes: IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES + 1
        }
      })
    ).toThrow(/sizeBytes/)
  })

  it('validates bounded canonical base64 and the deterministic display filename', () => {
    const output = {
      schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
      artifact,
      fileName: `generated-image-${sha256.slice(0, 12)}.png`,
      dataBase64: 'aGVsbG8='
    } as const
    expect(parseImageGenerationArtifactReadOutput(output)).toEqual(output)
    expect(() =>
      parseImageGenerationArtifactReadOutput({ ...output, dataBase64: 'aGVsbG8' })
    ).toThrow(/canonical base64/)
    expect(() =>
      parseImageGenerationArtifactReadOutput({ ...output, fileName: '../../private.png' })
    ).toThrow(/fileName/)
    expect(() =>
      parseImageGenerationArtifactReadOutput({
        ...output,
        managedPath: '/private/image-generation-artifacts/object.png'
      })
    ).toThrow(/unexpected field managedPath/)
  })

  it('reuses the read transport for a conversation-authorized PDF identity', () => {
    const documentArtifact = {
      artifactId: `sha256:${sha256}`,
      uri: `artifact://sha256/${sha256}`,
      kind: 'document',
      format: 'pdf',
      mimeType: 'application/pdf',
      sizeBytes: 5,
      sha256
    } as const
    const input = {
      schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
      artifact: documentArtifact,
      conversationId: 'conversation-1'
    } as const
    expect(parseImageGenerationArtifactReadInput(input)).toEqual(input)
    expect(
      parseImageGenerationArtifactReadOutput({
        schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
        artifact: documentArtifact,
        fileName: `artifact-${sha256.slice(0, 12)}.pdf`,
        dataBase64: 'aGVsbG8='
      })
    ).toEqual({
      schemaVersion: IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION,
      artifact: documentArtifact,
      fileName: `artifact-${sha256.slice(0, 12)}.pdf`,
      dataBase64: 'aGVsbG8='
    })
    expect(() =>
      parseImageGenerationArtifactReadInput({
        ...input,
        artifact: { ...documentArtifact, uri: `image-artifact://sha256/${sha256}` }
      })
    ).toThrow(/immutable document identity/)
    expect(() =>
      parseImageGenerationArtifactReadInput({
        ...input,
        artifact: { ...documentArtifact, sizeBytes: IMAGE_GENERATION_ARTIFACT_READ_MAX_BYTES + 1 }
      })
    ).toThrow(/sizeBytes/)
  })

  it('accepts only bounded path-free structured errors', () => {
    const error = {
      type: 'imageGenerationArtifact',
      operation: 'read',
      code: 'notFound',
      recovery: 'regenerate',
      message: 'The generated image is no longer available.',
      retryable: false
    } as const
    expect(parseImageGenerationArtifactErrorData(error)).toEqual(error)
    expect(() =>
      parseImageGenerationArtifactErrorData({
        ...error,
        providerUrl: 'https://provider.test/private'
      })
    ).toThrow(/unexpected field providerUrl/)
  })
})
