// Renderer Artifact boundary: only a trusted Host-backed resolver may provide preview bytes.
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { useEffect, useMemo, useState } from 'react'

export interface ResolvedImageArtifact {
  /** A Renderer-safe URL such as a Host-created object URL or controlled read-only scheme. */
  src: string
  /** Optional cleanup for object URLs or other temporary Renderer resources. */
  release?: () => void
  /** Retains the temporary resource for a longer-lived consumer such as the global viewer. */
  retain?: () => () => void
}

export interface ImageArtifactResolutionOptions {
  signal?: AbortSignal
}

export interface ImageArtifactResolver {
  resolve(
    artifact: AgentImageGenerationArtifact,
    options?: ImageArtifactResolutionOptions
  ): Promise<ResolvedImageArtifact>
}

export type ImageArtifactResolution =
  | { status: 'unavailable' }
  | { status: 'loading' }
  | { status: 'ready'; value: ResolvedImageArtifact }
  | { status: 'error' }

export function useImageArtifactResolutions(
  artifacts: readonly AgentImageGenerationArtifact[],
  resolver?: ImageArtifactResolver
): ReadonlyMap<string, ImageArtifactResolution> {
  const [resolutions, setResolutions] = useState<ReadonlyMap<string, ImageArtifactResolution>>(() =>
    initialResolutions(artifacts, resolver)
  )

  useEffect(() => {
    if (!resolver) {
      setResolutions(initialResolutions(artifacts))
      return undefined
    }

    let active = true
    const abortController = new AbortController()
    const resolvedValues = new Map<string, ResolvedImageArtifact>()
    setResolutions(initialResolutions(artifacts, resolver))

    for (const artifact of artifacts) {
      void resolver
        .resolve(artifact, { signal: abortController.signal })
        .then((value) => {
          if (!active) {
            value.release?.()
            return
          }
          resolvedValues.set(artifact.artifactId, value)
          setResolutions((current) =>
            updatedResolution(current, artifact.artifactId, { status: 'ready', value })
          )
        })
        .catch(() => {
          if (!active) return
          setResolutions((current) =>
            updatedResolution(current, artifact.artifactId, { status: 'error' })
          )
        })
    }

    return () => {
      active = false
      abortController.abort()
      resolvedValues.forEach((value) => value.release?.())
    }
  }, [artifacts, resolver])

  return resolutions
}

export function useImageArtifactResolution(
  artifact: AgentImageGenerationArtifact,
  resolver?: ImageArtifactResolver
): ImageArtifactResolution {
  const stableArtifact = useMemo<AgentImageGenerationArtifact>(
    () => ({
      artifactId: artifact.artifactId,
      uri: artifact.uri,
      kind: artifact.kind,
      format: artifact.format,
      mimeType: artifact.mimeType,
      width: artifact.width,
      height: artifact.height,
      sizeBytes: artifact.sizeBytes,
      sha256: artifact.sha256
    }),
    [
      artifact.artifactId,
      artifact.format,
      artifact.height,
      artifact.kind,
      artifact.mimeType,
      artifact.sha256,
      artifact.sizeBytes,
      artifact.uri,
      artifact.width
    ]
  )
  const [resolution, setResolution] = useState<ImageArtifactResolution>(() =>
    resolver ? { status: 'loading' } : { status: 'unavailable' }
  )

  useEffect(() => {
    if (!resolver) {
      setResolution({ status: 'unavailable' })
      return undefined
    }

    let active = true
    const abortController = new AbortController()
    let resolved: ResolvedImageArtifact | undefined
    setResolution({ status: 'loading' })
    void resolver
      .resolve(stableArtifact, { signal: abortController.signal })
      .then((value) => {
        resolved = value
        if (active) setResolution({ status: 'ready', value })
        else value.release?.()
      })
      .catch(() => {
        if (active) setResolution({ status: 'error' })
      })

    return () => {
      active = false
      abortController.abort()
      resolved?.release?.()
    }
  }, [resolver, stableArtifact])

  return resolution
}

function initialResolutions(
  artifacts: readonly AgentImageGenerationArtifact[],
  resolver?: ImageArtifactResolver
): ReadonlyMap<string, ImageArtifactResolution> {
  const status: ImageArtifactResolution = resolver
    ? { status: 'loading' }
    : { status: 'unavailable' }
  return new Map(artifacts.map((artifact) => [artifact.artifactId, status]))
}

function updatedResolution(
  current: ReadonlyMap<string, ImageArtifactResolution>,
  artifactId: string,
  resolution: ImageArtifactResolution
): ReadonlyMap<string, ImageArtifactResolution> {
  if (!current.has(artifactId)) return current
  return new Map(current).set(artifactId, resolution)
}
