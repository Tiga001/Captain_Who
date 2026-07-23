// Renderer result cards for successful managed image Artifacts; no private path is inferred.
import { Image as ImageIcon } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { ChatAgentRunView } from '../chatTypes'
import {
  getImageGenerationArtifactEntries,
  type ImageGenerationArtifactEntry
} from '../../imageGeneration/imageGenerationActivity'
import {
  type ImageArtifactResolver,
  useImageArtifactResolution
} from '../../imageGeneration/artifacts/ImageArtifactResolver'

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const kilobytes = bytes / 1024
  if (kilobytes < 1024) return `${kilobytes.toFixed(kilobytes >= 10 ? 0 : 1)} KB`
  const megabytes = kilobytes / 1024
  return `${megabytes.toFixed(megabytes >= 10 ? 0 : 1)} MB`
}

function ImageArtifactCard({
  entry,
  resolver
}: {
  entry: ImageGenerationArtifactEntry
  resolver?: ImageArtifactResolver
}) {
  const { t } = useFrontendConfig()
  const resolution = useImageArtifactResolution(entry.artifact, resolver)
  const format = entry.artifact.format === 'jpeg' ? 'JPEG' : entry.artifact.format.toUpperCase()
  const typeLabel = formatTranslation(t, 'agent.imageGeneration.artifactFormat', { format })
  const metadata = formatTranslation(t, 'agent.imageGeneration.artifactMetadata', {
    height: String(entry.artifact.height),
    size: formatBytes(entry.artifact.sizeBytes),
    width: String(entry.artifact.width)
  })
  const resolutionLabel =
    resolution.status === 'loading'
      ? t('agent.imageGeneration.previewLoading')
      : resolution.status === 'error'
        ? t('agent.imageGeneration.previewFailed')
        : t('agent.imageGeneration.previewUnavailable')

  return (
    <article className="image-generation-artifact-card">
      <span className="image-generation-artifact-card__preview" data-status={resolution.status}>
        {resolution.status === 'ready' ? (
          <img alt="" src={resolution.value.src} />
        ) : (
          <ImageIcon aria-hidden="true" />
        )}
      </span>
      <div className="image-generation-artifact-card__content">
        <p>{t('agent.imageGeneration.artifact')}</p>
        <span>{typeLabel}</span>
        <span>{metadata}</span>
        {resolution.status !== 'ready' ? <small>{resolutionLabel}</small> : null}
      </div>
    </article>
  )
}

export function ImageGenerationArtifactsCard({
  resolver,
  run
}: {
  resolver?: ImageArtifactResolver
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  const entries = getImageGenerationArtifactEntries(run)
  if (entries.length === 0) return null

  return (
    <section
      aria-label={t('agent.imageGeneration.artifacts')}
      className="image-generation-artifact-list"
    >
      {entries.map((entry) => (
        <ImageArtifactCard entry={entry} key={entry.artifact.artifactId} resolver={resolver} />
      ))}
    </section>
  )
}
