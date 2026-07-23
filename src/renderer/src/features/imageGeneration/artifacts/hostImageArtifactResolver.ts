// Production bridge from immutable image Artifact identities to Renderer-scoped object URLs.
import { HostInvocationError } from '@mycopilot/host-api'
import type { AgentImageGenerationArtifact } from '@mycopilot/protocol'
import { hostClient } from '../../../host/hostClient'
import type {
  ImageArtifactResolutionOptions,
  ImageArtifactResolver,
  ResolvedImageArtifact
} from './ImageArtifactResolver'

const MAX_CONCURRENT_ARTIFACT_READS = 2
let activeArtifactReads = 0
const artifactReadWaiters: ArtifactReadWaiter[] = []

interface ArtifactReadWaiter {
  admit: () => void
}

function abortError(): Error {
  return Object.assign(new Error('The image Artifact read was cancelled.'), { name: 'AbortError' })
}

async function acquireArtifactReadSlot(signal?: AbortSignal): Promise<() => void> {
  if (signal?.aborted) throw abortError()

  await new Promise<void>((resolve, reject) => {
    const admit = () => {
      signal?.removeEventListener('abort', cancel)
      activeArtifactReads += 1
      resolve()
    }
    const cancel = () => {
      const index = artifactReadWaiters.indexOf(waiter)
      if (index >= 0) artifactReadWaiters.splice(index, 1)
      reject(abortError())
    }
    const waiter: ArtifactReadWaiter = { admit }
    if (activeArtifactReads < MAX_CONCURRENT_ARTIFACT_READS) {
      admit()
    } else {
      artifactReadWaiters.push(waiter)
      signal?.addEventListener('abort', cancel, { once: true })
    }
  })

  let released = false
  return () => {
    if (released) return
    released = true
    activeArtifactReads -= 1
    artifactReadWaiters.shift()?.admit()
  }
}

function sameArtifact(
  left: AgentImageGenerationArtifact,
  right: AgentImageGenerationArtifact
): boolean {
  return (
    left.artifactId === right.artifactId &&
    left.uri === right.uri &&
    left.kind === right.kind &&
    left.format === right.format &&
    left.mimeType === right.mimeType &&
    left.width === right.width &&
    left.height === right.height &&
    left.sizeBytes === right.sizeBytes &&
    left.sha256 === right.sha256
  )
}

async function resolveHostImageArtifact(
  artifact: AgentImageGenerationArtifact,
  options?: ImageArtifactResolutionOptions
): Promise<ResolvedImageArtifact> {
  const releaseReadSlot = await acquireArtifactReadSlot(options?.signal)
  let result: Awaited<ReturnType<typeof hostClient.imageGeneration.readArtifact>>
  try {
    if (options?.signal?.aborted) throw abortError()
    result = await hostClient.imageGeneration.readArtifact({
      schemaVersion: 1,
      artifact
    })
  } finally {
    releaseReadSlot()
  }
  if (options?.signal?.aborted) throw abortError()
  if (!result.ok) throw new HostInvocationError(result.error)

  const content = result.value
  if (
    content.schemaVersion !== 1 ||
    !sameArtifact(content.artifact, artifact) ||
    !(content.bytes instanceof Uint8Array) ||
    content.bytes.byteLength !== artifact.sizeBytes
  ) {
    throw new Error('The generated image Artifact response is invalid.')
  }

  // Own a Renderer-local copy so the IPC transfer buffer cannot be mutated after resolution.
  const bytes = Uint8Array.from(content.bytes)
  const src = URL.createObjectURL(new Blob([bytes], { type: artifact.mimeType }))
  let referenceCount = 1
  let revoked = false
  let baseRetained = true
  const releaseReference = () => {
    if (revoked || referenceCount === 0) return
    referenceCount -= 1
    if (referenceCount === 0) {
      revoked = true
      URL.revokeObjectURL(src)
    }
  }
  return {
    src,
    release: () => {
      if (!baseRetained) return
      baseRetained = false
      releaseReference()
    },
    retain: () => {
      if (revoked) throw new Error('The generated image preview is no longer available.')
      referenceCount += 1
      let retained = true
      return () => {
        if (!retained) return
        retained = false
        releaseReference()
      }
    }
  }
}

/** The only production resolver for private generated-image Artifacts. */
export const hostImageArtifactResolver: ImageArtifactResolver = {
  resolve: resolveHostImageArtifact
}
