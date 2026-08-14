// Renderer Artifact boundary: only a trusted Host-backed resolver may provide preview bytes.
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { useEffect, useMemo, useRef, useState } from 'react'

export interface ResolvedImageArtifact {
  /** A Renderer-safe URL such as a Host-created object URL or controlled read-only scheme. */
  src: string
  /** Optional cleanup for object URLs or other temporary Renderer resources. */
  release?: () => void
  /** Retains the temporary resource for a longer-lived consumer such as the global viewer. */
  retain?: () => () => void
}

export interface ImageArtifactResolutionOptions {
  conversationId?: string
  observerRootConversationId?: string
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

interface ActiveImageArtifactRequest {
  abortController: AbortController
  disposed: boolean
  resolutions: ReadonlyMap<string, ImageArtifactResolution>
  resolvedValues: Map<string, ResolvedImageArtifact>
  resolver?: ImageArtifactResolver
  signature: string
}

interface ImageArtifactResolutionSnapshot {
  request?: ActiveImageArtifactRequest
  resolutions: ReadonlyMap<string, ImageArtifactResolution>
}

interface ImageArtifactRequestDescriptor {
  artifacts: readonly AgentImageGenerationArtifact[]
  conversationId?: string
  observerRootConversationId?: string
}

type SerializedImageArtifact = [
  artifactId: string,
  uri: string,
  kind: AgentImageGenerationArtifact['kind'],
  format: AgentImageGenerationArtifact['format'],
  mimeType: string,
  width: number,
  height: number,
  sizeBytes: number,
  sha256: string
]

export function useImageArtifactResolutions(
  artifacts: readonly AgentImageGenerationArtifact[],
  resolver?: ImageArtifactResolver,
  conversationId?: string,
  observerRootConversationId?: string
): ReadonlyMap<string, ImageArtifactResolution> {
  const requestSignature = imageArtifactRequestSignature(
    artifacts,
    conversationId,
    observerRootConversationId
  )
  const requestDescriptor = useMemo(
    () => imageArtifactRequestDescriptor(requestSignature),
    [requestSignature]
  )
  const activeRequestRef = useRef<ActiveImageArtifactRequest | undefined>(undefined)
  const [snapshot, setSnapshot] = useState<ImageArtifactResolutionSnapshot>(() => ({
    resolutions: initialResolutions(artifacts, resolver)
  }))

  // Reconcile after every render, but replace the request only when its semantic authority,
  // resolver, or immutable Artifact fingerprint changes. Restored run objects and appended
  // Timeline events are intentionally invisible to this lifecycle.
  useEffect(() => {
    const current = activeRequestRef.current
    if (
      current &&
      !current.disposed &&
      current.signature === requestSignature &&
      current.resolver === resolver
    ) {
      return
    }

    if (current) disposeImageArtifactRequest(current)

    const request: ActiveImageArtifactRequest = {
      abortController: new AbortController(),
      disposed: false,
      resolutions: initialResolutions(requestDescriptor.artifacts, resolver),
      resolvedValues: new Map(),
      resolver,
      signature: requestSignature
    }
    activeRequestRef.current = request
    setSnapshot({ request, resolutions: request.resolutions })

    if (!resolver) return

    for (const artifact of requestDescriptor.artifacts) {
      void resolver
        .resolve(artifact, {
          signal: request.abortController.signal,
          ...(requestDescriptor.conversationId
            ? { conversationId: requestDescriptor.conversationId }
            : {}),
          ...(requestDescriptor.observerRootConversationId
            ? { observerRootConversationId: requestDescriptor.observerRootConversationId }
            : {})
        })
        .then((value) => {
          if (request.disposed || activeRequestRef.current !== request) {
            value.release?.()
            return
          }
          request.resolvedValues.set(artifact.artifactId, value)
          request.resolutions = updatedResolution(request.resolutions, artifact.artifactId, {
            status: 'ready',
            value
          })
          setSnapshot((currentSnapshot) =>
            currentSnapshot.request === request
              ? { request, resolutions: request.resolutions }
              : currentSnapshot
          )
        })
        .catch(() => {
          if (request.disposed || activeRequestRef.current !== request) return
          request.resolutions = updatedResolution(request.resolutions, artifact.artifactId, {
            status: 'error'
          })
          setSnapshot((currentSnapshot) =>
            currentSnapshot.request === request
              ? { request, resolutions: request.resolutions }
              : currentSnapshot
          )
        })
    }
  }, [requestDescriptor, requestSignature, resolver])

  useEffect(() => {
    return () => {
      const request = activeRequestRef.current
      if (!request) return
      disposeImageArtifactRequest(request)
      if (activeRequestRef.current === request) activeRequestRef.current = undefined
    }
  }, [])

  if (snapshot.request?.signature !== requestSignature || snapshot.request.resolver !== resolver) {
    // Never render bytes retained under a previous Conversation/observer authority while the
    // effect is switching scope. The new request will publish loading/unavailable synchronously
    // on its first reconciliation pass.
    return initialResolutions(artifacts, resolver)
  }
  return snapshot.resolutions
}

function disposeImageArtifactRequest(request: ActiveImageArtifactRequest) {
  if (request.disposed) return
  request.disposed = true
  request.abortController.abort()
  request.resolvedValues.forEach((value) => value.release?.())
  request.resolvedValues.clear()
}

function imageArtifactRequestSignature(
  artifacts: readonly AgentImageGenerationArtifact[],
  conversationId?: string,
  observerRootConversationId?: string
): string {
  return JSON.stringify([
    conversationId || null,
    observerRootConversationId || null,
    artifacts.map((artifact) => [
      artifact.artifactId,
      artifact.uri,
      artifact.kind,
      artifact.format,
      artifact.mimeType,
      artifact.width,
      artifact.height,
      artifact.sizeBytes,
      artifact.sha256
    ])
  ])
}

function imageArtifactRequestDescriptor(signature: string): ImageArtifactRequestDescriptor {
  const [conversationId, observerRootConversationId, serializedArtifacts] = JSON.parse(
    signature
  ) as [string | null, string | null, SerializedImageArtifact[]]
  return {
    artifacts: serializedArtifacts.map(
      ([artifactId, uri, kind, format, mimeType, width, height, sizeBytes, sha256]) => ({
        artifactId,
        uri,
        kind,
        format,
        mimeType,
        width,
        height,
        sizeBytes,
        sha256
      })
    ),
    ...(conversationId ? { conversationId } : {}),
    ...(observerRootConversationId ? { observerRootConversationId } : {})
  }
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
