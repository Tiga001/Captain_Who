// Renderer image-generation activity: presents one safe, callId-stable card for every Tool call.
import {
  CircleHelp,
  CircleSlash2,
  ImageOff,
  Images,
  LoaderCircle,
  type LucideIcon
} from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import {
  getImageGenerationArtifactEntry,
  getImageGenerationActivityView,
  type ImageGenerationArtifactEntry,
  type ImageGenerationActivityOperation,
  type ImageGenerationActivityStatus
} from '../../../imageGeneration/imageGenerationActivity'
import {
  type ImageArtifactResolver,
  type ResolvedImageArtifact,
  useImageArtifactResolutions
} from '../../../imageGeneration/artifacts/ImageArtifactResolver'
import { useImagePreview } from '../ImagePreview'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

const IMAGE_ARTIFACT_PRELOAD_MARGIN_PX = 800
const lazyHostImageArtifactResolver: ImageArtifactResolver = {
  async resolve(artifact, options) {
    const { hostImageArtifactResolver } =
      await import('../../../imageGeneration/artifacts/hostImageArtifactResolver')
    return hostImageArtifactResolver.resolve(artifact, options)
  }
}

const LABEL_KEY: Readonly<
  Record<ImageGenerationActivityOperation, Record<ImageGenerationActivityStatus, TranslationKey>>
> = {
  generate: {
    running: 'agent.imageGeneration.generate.running',
    completed: 'agent.imageGeneration.generate.completed',
    failed: 'agent.imageGeneration.generate.failed',
    cancelled: 'agent.imageGeneration.generate.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.generate.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.generate.commitIndeterminate'
  },
  edit: {
    running: 'agent.imageGeneration.edit.running',
    completed: 'agent.imageGeneration.edit.completed',
    failed: 'agent.imageGeneration.edit.failed',
    cancelled: 'agent.imageGeneration.edit.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.edit.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.edit.commitIndeterminate'
  },
  unknown: {
    running: 'agent.imageGeneration.unknown.running',
    completed: 'agent.imageGeneration.unknown.completed',
    failed: 'agent.imageGeneration.unknown.failed',
    cancelled: 'agent.imageGeneration.unknown.cancelled',
    outcomeIndeterminate: 'agent.imageGeneration.unknown.outcomeIndeterminate',
    commitIndeterminate: 'agent.imageGeneration.unknown.commitIndeterminate'
  }
}

const STATUS_ICON: Readonly<Record<ImageGenerationActivityStatus, LucideIcon>> = {
  running: LoaderCircle,
  completed: Images,
  failed: ImageOff,
  cancelled: CircleSlash2,
  outcomeIndeterminate: CircleHelp,
  commitIndeterminate: CircleHelp
}

export function ImageGenerationToolActivity({
  call,
  resolver = lazyHostImageArtifactResolver,
  result,
  settledStatus
}: {
  call: AgentToolCall
  resolver?: ImageArtifactResolver
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}) {
  const { t } = useFrontendConfig()
  const view = getImageGenerationActivityView(call, result, settledStatus)
  const artifactEntry = useMemo(() => getImageGenerationArtifactEntry(call, result), [call, result])
  const Icon = STATUS_ICON[view.status]
  const safeFailure = view.status === 'failed' ? t('agent.imageGeneration.safeFailure') : undefined
  const details = [view.reason, view.failureMessage ?? safeFailure, view.failureRecovery].filter(
    (value, index, values): value is string => Boolean(value && values.indexOf(value) === index)
  )
  const showPreview = view.status === 'running' || Boolean(artifactEntry)

  return (
    <AgentActivityDisclosure
      className="agent-activity--image-generation"
      defaultOpen={showPreview}
      hasDetails={details.length > 0 || showPreview}
      icon={Icon}
      isPending={view.status === 'running'}
      label={t(LABEL_KEY[view.operation][view.status])}
    >
      {details.length > 0 || showPreview ? (
        <div className="agent-activity__details image-generation-activity__details">
          {showPreview ? (
            <ImageGenerationActivityPreview
              entry={artifactEntry}
              resolver={resolver}
              running={view.status === 'running'}
            />
          ) : null}
          {details.map((detail) => (
            <p key={detail}>{detail}</p>
          ))}
        </div>
      ) : null}
    </AgentActivityDisclosure>
  )
}

function ImageGenerationActivityPreview({
  entry,
  resolver,
  running
}: {
  entry?: ImageGenerationArtifactEntry
  resolver?: ImageArtifactResolver
  running: boolean
}) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const { elementRef, isNearViewport } = useNearViewportOnce()
  const [failedSource, setFailedSource] = useState<string>()
  const artifacts = useMemo(() => (entry ? [entry.artifact] : []), [entry])
  const resolutions = useImageArtifactResolutions(
    artifacts,
    entry && isNearViewport ? resolver : undefined
  )
  const resolution = entry
    ? (resolutions.get(entry.artifact.artifactId) ?? { status: 'unavailable' as const })
    : undefined
  const resolvedValue = resolution?.status === 'ready' ? resolution.value : undefined
  const readyValue = resolvedValue && failedSource !== resolvedValue.src ? resolvedValue : undefined
  const status = readyValue
    ? 'ready'
    : running || (entry && (!isNearViewport || resolution?.status === 'loading'))
      ? 'loading'
      : 'error'
  const fileName = entry ? artifactFileName(entry) : undefined

  useEffect(() => {
    setFailedSource(undefined)
  }, [resolvedValue?.src])

  const openArtifact = (value: ResolvedImageArtifact) => {
    const release = value.retain?.()
    openImagePreview({
      alt: t('agent.imageGeneration.artifact'),
      fileName,
      src: value.src,
      ...(release ? { release } : {})
    })
  }

  return (
    <div className="image-generation-activity__preview" data-status={status} ref={elementRef}>
      {readyValue ? (
        <button
          aria-label={t('imagePreview.title')}
          onClick={() => openArtifact(readyValue)}
          type="button"
        >
          <img
            alt={fileName}
            draggable={false}
            onError={() => setFailedSource(readyValue.src)}
            src={readyValue.src}
          />
        </button>
      ) : status === 'loading' ? (
        <span aria-hidden="true" className="image-generation-activity__wave">
          <i />
          <i />
          <i />
        </span>
      ) : (
        <ImageOff aria-hidden="true" />
      )}
    </div>
  )
}

function useNearViewportOnce() {
  const elementRef = useRef<HTMLDivElement>(null)
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
        // This is a one-way preload gate: ordinary scrolling must not release and reload the image.
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

function artifactFileName(entry: ImageGenerationArtifactEntry): string {
  const extension = entry.artifact.format === 'jpeg' ? 'jpg' : entry.artifact.format
  return `generated-image-${entry.artifact.sha256.slice(0, 12)}.${extension}`
}
