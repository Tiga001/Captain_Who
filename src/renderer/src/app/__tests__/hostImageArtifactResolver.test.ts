import type { HostInvocationResult, ImageGenerationArtifactContent } from '@mycopilot/host-api'
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  readArtifact: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    imageGeneration: {
      readArtifact: mocks.readArtifact
    }
  }
}))

import { hostImageArtifactResolver } from '../../features/imageGeneration/artifacts/hostImageArtifactResolver'

const bytes = Uint8Array.from([137, 80, 78, 71])
const sha256 = 'a'.repeat(64)
const artifact = {
  artifactId: `sha256:${sha256}`,
  uri: `image-artifact://sha256/${sha256}`,
  kind: 'image',
  format: 'png',
  mimeType: 'image/png',
  width: 1,
  height: 1,
  sizeBytes: bytes.byteLength,
  sha256
} satisfies AgentImageGenerationArtifact

describe('Host image Artifact resolver', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    mocks.readArtifact.mockReset()
  })

  it('creates one revocable object URL from verified Host bytes', async () => {
    const createObjectURL = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:artifact')
    const revokeObjectURL = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    mocks.readArtifact.mockResolvedValue({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact,
        fileName: `generated-image-${sha256.slice(0, 12)}.png`,
        bytes
      }
    } satisfies HostInvocationResult<ImageGenerationArtifactContent>)

    const resolved = await hostImageArtifactResolver.resolve(artifact)

    expect(mocks.readArtifact).toHaveBeenCalledWith({ schemaVersion: 1, artifact })
    expect(createObjectURL).toHaveBeenCalledOnce()
    expect(createObjectURL.mock.calls[0]?.[0]).toBeInstanceOf(Blob)
    expect(resolved.src).toBe('blob:artifact')

    const releaseViewerLease = resolved.retain?.()
    resolved.release?.()
    resolved.release?.()
    expect(revokeObjectURL).not.toHaveBeenCalled()
    releaseViewerLease?.()
    releaseViewerLease?.()
    expect(revokeObjectURL).toHaveBeenCalledOnce()
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:artifact')
  })

  it('rejects structured Host failures and changed frozen identity', async () => {
    mocks.readArtifact.mockResolvedValueOnce({
      ok: false,
      error: {
        message: 'The generated image is no longer available.',
        code: -32021
      }
    })
    await expect(hostImageArtifactResolver.resolve(artifact)).rejects.toMatchObject({
      message: 'The generated image is no longer available.',
      code: -32021
    })

    mocks.readArtifact.mockResolvedValueOnce({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact: { ...artifact, width: 2 },
        fileName: `generated-image-${sha256.slice(0, 12)}.png`,
        bytes
      }
    })
    await expect(hostImageArtifactResolver.resolve(artifact)).rejects.toThrow(
      /Artifact response is invalid/
    )
  })

  it('globally bounds concurrent Host reads so multi-image history does not hit admission errors', async () => {
    const pending: Array<(value: HostInvocationResult<ImageGenerationArtifactContent>) => void> = []
    vi.spyOn(URL, 'createObjectURL').mockImplementation(() => `blob:artifact-${pending.length}`)
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    mocks.readArtifact.mockImplementation(
      () =>
        new Promise<HostInvocationResult<ImageGenerationArtifactContent>>((resolve) => {
          pending.push(resolve)
        })
    )

    const first = hostImageArtifactResolver.resolve(artifact)
    const second = hostImageArtifactResolver.resolve(artifact)
    const third = hostImageArtifactResolver.resolve(artifact)
    await vi.waitFor(() => expect(mocks.readArtifact).toHaveBeenCalledTimes(2))

    pending[0]?.({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact,
        fileName: `generated-image-${sha256.slice(0, 12)}.png`,
        bytes
      }
    })
    await vi.waitFor(() => expect(mocks.readArtifact).toHaveBeenCalledTimes(3))

    pending[1]?.({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact,
        fileName: `generated-image-${sha256.slice(0, 12)}.png`,
        bytes
      }
    })
    pending[2]?.({
      ok: true,
      value: {
        schemaVersion: 1,
        artifact,
        fileName: `generated-image-${sha256.slice(0, 12)}.png`,
        bytes
      }
    })

    const resolved = await Promise.all([first, second, third])
    resolved.forEach((value) => value.release?.())
  })

  it('removes cancelled queued reads before they reach the Host', async () => {
    const pending: Array<(value: HostInvocationResult<ImageGenerationArtifactContent>) => void> = []
    vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:artifact')
    vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => undefined)
    mocks.readArtifact.mockImplementation(
      () =>
        new Promise<HostInvocationResult<ImageGenerationArtifactContent>>((resolve) => {
          pending.push(resolve)
        })
    )

    const first = hostImageArtifactResolver.resolve(artifact)
    const second = hostImageArtifactResolver.resolve(artifact)
    const abortController = new AbortController()
    const cancelled = hostImageArtifactResolver.resolve(artifact, {
      signal: abortController.signal
    })
    await vi.waitFor(() => expect(mocks.readArtifact).toHaveBeenCalledTimes(2))
    abortController.abort()
    await expect(cancelled).rejects.toMatchObject({ name: 'AbortError' })
    expect(mocks.readArtifact).toHaveBeenCalledTimes(2)

    for (const resolve of pending) {
      resolve({
        ok: true,
        value: {
          schemaVersion: 1,
          artifact,
          fileName: `generated-image-${sha256.slice(0, 12)}.png`,
          bytes
        }
      })
    }
    const resolved = await Promise.all([first, second])
    resolved.forEach((value) => value.release?.())
  })
})
