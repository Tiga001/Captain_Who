// Renderer Artifact boundary: only a trusted Host-backed resolver may provide preview bytes.
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { useEffect, useState } from 'react'

export interface ResolvedImageArtifact {
  /** A Renderer-safe URL such as a Host-created object URL or controlled read-only scheme. */
  src: string
  /** Optional cleanup for object URLs or other temporary Renderer resources. */
  release?: () => void
}

export interface ImageArtifactResolver {
  resolve(artifact: AgentImageGenerationArtifact): Promise<ResolvedImageArtifact>
}

export type ImageArtifactResolution =
  | { status: 'unavailable' }
  | { status: 'loading' }
  | { status: 'ready'; value: ResolvedImageArtifact }
  | { status: 'error' }

export function useImageArtifactResolution(
  artifact: AgentImageGenerationArtifact,
  resolver?: ImageArtifactResolver
): ImageArtifactResolution {
  const [resolution, setResolution] = useState<ImageArtifactResolution>(() =>
    resolver ? { status: 'loading' } : { status: 'unavailable' }
  )

  useEffect(() => {
    if (!resolver) {
      setResolution({ status: 'unavailable' })
      return undefined
    }

    let active = true
    let resolved: ResolvedImageArtifact | undefined
    setResolution({ status: 'loading' })
    void resolver
      .resolve(artifact)
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
      resolved?.release?.()
    }
  }, [artifact, resolver])

  return resolution
}
