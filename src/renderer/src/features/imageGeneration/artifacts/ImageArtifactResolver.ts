// Renderer Artifact boundary: only a trusted Host-backed resolver may provide preview bytes.
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { useEffect, useState } from 'react'

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
