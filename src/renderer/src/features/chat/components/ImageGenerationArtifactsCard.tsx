// Renderer result cards for successful managed image Artifacts; no private path is inferred.
import { Image as ImageIcon } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getImageGenerationArtifactEntries,
  type ImageGenerationArtifactEntry
} from '../../imageGeneration/imageGenerationActivity'
import {
  type ImageArtifactResolver,
  type ImageArtifactResolution,
  type ResolvedImageArtifact,
  useImageArtifactResolutions
} from '../../imageGeneration/artifacts/ImageArtifactResolver'
import { useImagePreview } from './ImagePreview'

const IMAGE_ARTIFACT_PRELOAD_MARGIN_PX = 800

function useNearViewport() {
  const elementRef = useRef<HTMLElement>(null)
  const [isNearViewport, setIsNearViewport] = useState(false)

  useEffect(() => {
    const element = elementRef.current
    if (!element) return undefined
    if (typeof IntersectionObserver === 'undefined') {
      setIsNearViewport(true)
      return undefined
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return
        // Visibility is only a preload gate. Once bytes have been resolved, keep the object URL
        // alive until this message actually unmounts so ordinary scrolling never reloads it.
        setIsNearViewport(true)
        observer.disconnect()
      },
      { rootMargin: `${IMAGE_ARTIFACT_PRELOAD_MARGIN_PX}px 0px` }
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  return { elementRef, isNearViewport }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const kilobytes = bytes / 1024
  if (kilobytes < 1024) return `${kilobytes.toFixed(kilobytes >= 10 ? 0 : 1)} KB`
  const megabytes = kilobytes / 1024
  return `${megabytes.toFixed(megabytes >= 10 ? 0 : 1)} MB`
}

function artifactFileName(entry: ImageGenerationArtifactEntry): string {
  if (entry.displayName) return entry.displayName
  const extension = entry.artifact.format === 'jpeg' ? 'jpg' : entry.artifact.format
  return `generated-image-${entry.artifact.sha256.slice(0, 12)}.${extension}`
}

function resolutionLabel(
  resolution: ImageArtifactResolution,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  if (resolution.status === 'loading') return t('agent.imageGeneration.previewLoading')
  if (resolution.status === 'error') return t('agent.imageGeneration.previewFailed')
  return t('agent.imageGeneration.previewUnavailable')
}

function ImageArtifactPreview({
  entry,
  onOpen,
  resolution
}: {
  entry: ImageGenerationArtifactEntry
  onOpen: (entry: ImageGenerationArtifactEntry, value: ResolvedImageArtifact) => void
  resolution: ImageArtifactResolution
}) {
  const { t } = useFrontendConfig()
  const previewStyle = {
    '--image-artifact-aspect-ratio': `${entry.artifact.width} / ${entry.artifact.height}`
  } as CSSProperties
  const resolvedValue = resolution.status === 'ready' ? resolution.value : undefined
  const previewSrc = resolvedValue?.src

  return (
    <div
      className="image-generation-artifact-preview"
      data-status={resolution.status}
      style={previewStyle}
    >
      {resolvedValue ? (
        <button
          aria-label={t('imagePreview.title')}
          onClick={() => onOpen(entry, resolvedValue)}
          type="button"
        >
          <img alt={artifactFileName(entry)} draggable={false} src={previewSrc} />
        </button>
      ) : (
        <ImageIcon aria-hidden="true" />
      )}
    </div>
  )
}

function ImageArtifactRow({
  entry,
  onOpen,
  resolution
}: {
  entry: ImageGenerationArtifactEntry
  onOpen: (entry: ImageGenerationArtifactEntry, value: ResolvedImageArtifact) => void
  resolution: ImageArtifactResolution
}) {
  const { t } = useFrontendConfig()
  const format = entry.artifact.format === 'jpeg' ? 'JPEG' : entry.artifact.format.toUpperCase()
  const typeLabel = formatTranslation(t, 'agent.imageGeneration.artifactFormat', { format })
  const metadata = formatTranslation(t, 'agent.imageGeneration.artifactMetadata', {
    height: String(entry.artifact.height),
    size: formatBytes(entry.artifact.sizeBytes),
    width: String(entry.artifact.width)
  })
  const resolvedValue = resolution.status === 'ready' ? resolution.value : undefined
  const previewSrc = resolvedValue?.src
  const content = (
    <>
      <span className="image-generation-artifact-card__icon">
        {previewSrc ? (
          <img alt="" aria-hidden="true" draggable={false} src={previewSrc} />
        ) : (
          <ImageIcon aria-hidden="true" />
        )}
      </span>
      <span className="image-generation-artifact-card__content">
        <span className="image-generation-artifact-card__name">{artifactFileName(entry)}</span>
        <span>
          {typeLabel} · {metadata}
        </span>
        {previewSrc ? null : <small>{resolutionLabel(resolution, t)}</small>}
      </span>
    </>
  )

  return (
    <article className="image-generation-artifact-card">
      {resolvedValue ? (
        <button onClick={() => onOpen(entry, resolvedValue)} type="button">
          {content}
        </button>
      ) : (
        <div>{content}</div>
      )}
    </article>
  )
}

export function ImageGenerationArtifactsCard({
  conversationId,
  observerRootConversationId,
  resolver,
  run
}: {
  conversationId?: string
  observerRootConversationId?: string
  resolver?: ImageArtifactResolver
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const { elementRef, isNearViewport } = useNearViewport()
  const [failedSources, setFailedSources] = useState<ReadonlySet<string>>(() => new Set())
  const entries = useMemo(() => getImageGenerationArtifactEntries(run), [run])
  const artifacts = useMemo(() => entries.map((entry) => entry.artifact), [entries])
  const resolutions = useImageArtifactResolutions(
    artifacts,
    isNearViewport ? resolver : undefined,
    conversationId,
    observerRootConversationId
  )
  if (entries.length === 0) return null

  const effectiveResolution = (entry: ImageGenerationArtifactEntry): ImageArtifactResolution => {
    if (resolver && !isNearViewport) return { status: 'loading' }
    const resolution = resolutions.get(entry.artifact.artifactId) ?? { status: 'unavailable' }
    if (
      resolution.status === 'ready' &&
      failedSources.has(`${entry.artifact.artifactId}\u0000${resolution.value.src}`)
    ) {
      return { status: 'error' }
    }
    return resolution
  }
  const openArtifact = (entry: ImageGenerationArtifactEntry, value: ResolvedImageArtifact) => {
    const release = value.retain?.()
    openImagePreview({
      alt: t('agent.imageGeneration.artifact'),
      fileName: artifactFileName(entry),
      src: value.src,
      ...(release ? { release } : {})
    })
  }

  return (
    <section
      aria-label={t('agent.imageGeneration.artifacts')}
      className="image-generation-artifact-section"
      data-count={entries.length}
      ref={elementRef}
    >
      <div className="image-generation-artifact-gallery">
        {entries.map((entry) => {
          const resolution = effectiveResolution(entry)
          return (
            <div
              key={entry.artifact.artifactId}
              onErrorCapture={(event) => {
                if (event.target instanceof HTMLImageElement && resolution.status === 'ready') {
                  const failedKey = `${entry.artifact.artifactId}\u0000${resolution.value.src}`
                  setFailedSources((current) => new Set(current).add(failedKey))
                }
              }}
            >
              <ImageArtifactPreview entry={entry} onOpen={openArtifact} resolution={resolution} />
            </div>
          )
        })}
      </div>
      <div className="image-generation-artifact-list">
        {entries.map((entry) => (
          <ImageArtifactRow
            entry={entry}
            key={entry.artifact.artifactId}
            onOpen={openArtifact}
            resolution={effectiveResolution(entry)}
          />
        ))}
      </div>
    </section>
  )
}
