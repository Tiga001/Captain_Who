import { FileSpreadsheet, FileText, FileType, ImageIcon, Presentation } from 'lucide-react'
import {
  parseAgentImageGenerationArtifact,
  type AgentImageGenerationArtifact,
  type AgentToolCall,
  type AgentToolResult
} from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import type {
  ImageArtifactResolver,
  ResolvedImageArtifact
} from '../../../imageGeneration/artifacts/ImageArtifactResolver'
import { loadImageFile } from '../../../storage/storageClient'
import { normalizeReadImageThumbnailDataUrl } from '../../agentReadActivities'
import type { ChatReadActivity, ChatReadActivityKind } from '../../chatTypes'
import { useImagePreview, useImagePreviewNotice } from '../ImagePreview'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface ReadToolActivityProps {
  activity?: ChatReadActivity
  artifactResolver?: ImageArtifactResolver
  call: AgentToolCall
  projectId?: string | null
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface ReadToolActivityGroupItem extends ReadToolActivityProps {}

interface ReadToolActivityGroupProps {
  items: ReadToolActivityGroupItem[]
  projectId?: string | null
}

const lazyHostImageArtifactResolver: ImageArtifactResolver = {
  async resolve(artifact, options) {
    const { hostImageArtifactResolver } =
      await import('../../../imageGeneration/artifacts/hostImageArtifactResolver')
    return hostImageArtifactResolver.resolve(artifact, options)
  }
}

const TOOL_KINDS: Partial<Record<string, ChatReadActivityKind>> = {
  read_file: 'file',
  read_image: 'image',
  read_presentation: 'presentation',
  read_spreadsheet: 'spreadsheet',
  read_word: 'word'
}

const PATH_IS_DIRECTORY_CODE = 'path_is_directory'

const STATUS_LABELS: Record<
  ChatReadActivityKind,
  {
    running: TranslationKey
    completed: TranslationKey
    failed: TranslationKey
    cancelled: TranslationKey
  }
> = {
  file: {
    running: 'agent.read.file.running',
    completed: 'agent.read.file.completed',
    failed: 'agent.read.file.failed',
    cancelled: 'agent.read.file.cancelled'
  },
  image: {
    running: 'agent.read.image.running',
    completed: 'agent.read.image.completed',
    failed: 'agent.read.image.failed',
    cancelled: 'agent.read.image.cancelled'
  },
  word: {
    running: 'agent.read.word.running',
    completed: 'agent.read.word.completed',
    failed: 'agent.read.word.failed',
    cancelled: 'agent.read.word.cancelled'
  },
  presentation: {
    running: 'agent.read.presentation.running',
    completed: 'agent.read.presentation.completed',
    failed: 'agent.read.presentation.failed',
    cancelled: 'agent.read.presentation.cancelled'
  },
  spreadsheet: {
    running: 'agent.read.spreadsheet.running',
    completed: 'agent.read.spreadsheet.completed',
    failed: 'agent.read.spreadsheet.failed',
    cancelled: 'agent.read.spreadsheet.cancelled'
  }
}

function getKind(call: AgentToolCall, activity: ChatReadActivity | undefined) {
  return activity?.kind ?? TOOL_KINDS[call.tool] ?? 'file'
}

function getStatus(
  activity: ChatReadActivity | undefined,
  result: AgentToolResult | undefined,
  settledStatus?: SettledToolStatus
) {
  if (result) return result.ok ? 'completed' : 'failed'
  if (activity?.status && activity.status !== 'running') return activity.status
  if (settledStatus) return settledStatus
  if (activity?.status) return activity.status
  return 'running'
}

function getPathFromCall(call: AgentToolCall) {
  if (!call.args || typeof call.args !== 'object' || Array.isArray(call.args)) return ''
  const args = call.args as Record<string, unknown>
  const path = typeof args.path === 'string' ? args.path : ''
  const filePath = typeof args.filePath === 'string' ? args.filePath : ''
  return path.trim() || filePath.trim()
}

function getResultRecord(result: AgentToolResult | undefined): Record<string, unknown> | undefined {
  if (!result?.result || typeof result.result !== 'object' || Array.isArray(result.result)) {
    return undefined
  }
  return result.result as Record<string, unknown>
}

function getPathFromResult(result: AgentToolResult | undefined) {
  const path = getResultRecord(result)?.path
  return typeof path === 'string' ? path.trim() : ''
}

function isPathIsDirectoryFailure(call: AgentToolCall, result: AgentToolResult | undefined) {
  return (
    call.tool === 'read_file' &&
    result?.ok === false &&
    getResultRecord(result)?.code === PATH_IS_DIRECTORY_CODE
  )
}

function getDisplayPath(
  activity: ChatReadActivity | undefined,
  call: AgentToolCall,
  result: AgentToolResult | undefined
) {
  return getPathFromResult(result) || activity?.path?.trim() || getPathFromCall(call)
}

function getFileName(path: string) {
  const normalized = path.replace(/\\/g, '/').replace(/\/$/, '')
  return normalized.split('/').filter(Boolean).pop() || normalized
}

function generatedArtifactFromResult(
  sourcePath: string,
  result: AgentToolResult | undefined
): AgentImageGenerationArtifact | undefined {
  if (
    !sourcePath.startsWith('image-artifact://sha256/') &&
    !sourcePath.startsWith('artifact://sha256/')
  ) {
    return undefined
  }
  if (!result?.ok || !result.result || typeof result.result !== 'object') return undefined
  const artifact = (result.result as Record<string, unknown>).artifact
  try {
    const parsed = parseAgentImageGenerationArtifact(artifact, 'read_image result.artifact')
    return parsed.uri === sourcePath ? parsed : undefined
  } catch {
    return undefined
  }
}

function artifactFileName(artifact: AgentImageGenerationArtifact): string {
  const extension = artifact.format === 'jpeg' ? 'jpg' : artifact.format
  return `generated-image-${artifact.sha256.slice(0, 12)}.${extension}`
}

function openResolvedArtifact(
  artifact: AgentImageGenerationArtifact,
  value: ResolvedImageArtifact,
  openImagePreview: ReturnType<typeof useImagePreview>
) {
  const retainedRelease = value.retain?.()
  if (retainedRelease) value.release?.()
  const fileName = artifactFileName(artifact)
  openImagePreview({
    alt: fileName,
    fileName,
    src: value.src,
    ...(retainedRelease
      ? { release: retainedRelease }
      : value.release
        ? { release: value.release }
        : {})
  })
}

function getDisplayName(activity: ChatReadActivity | undefined, call: AgentToolCall, t: Translate) {
  if (activity?.fileName) return activity.fileName
  return getFileName(getPathFromCall(call)) || t('agent.read.fallbackFile')
}

function getStatusIcon(kind: ChatReadActivityKind) {
  if (kind === 'image') return ImageIcon
  if (kind === 'spreadsheet') return FileSpreadsheet
  if (kind === 'presentation') return Presentation
  if (kind === 'word') return FileType
  return FileText
}

function imageDataUrlFromRecord(image: { data: string; mimeType: string }) {
  return `data:${image.mimeType};base64,${image.data}`
}

function getReadCountLabel(t: Translate, kind: ChatReadActivityKind, count: number) {
  return formatTranslation(t, `agent.read.count.${kind}` as TranslationKey, { count })
}

function getReadGroupLabel(
  t: Translate,
  kind: ChatReadActivityKind,
  items: ReadToolActivityGroupItem[]
) {
  const counts = items.reduce(
    (currentCounts, item) => {
      const status = getStatus(item.activity, item.result, item.settledStatus)
      currentCounts[status] += 1
      if (status === 'failed' && isPathIsDirectoryFailure(item.call, item.result)) {
        currentCounts.pathFailed += 1
      }
      return currentCounts
    },
    { cancelled: 0, completed: 0, failed: 0, pathFailed: 0, running: 0 }
  )
  const otherFailed = counts.failed - counts.pathFailed

  if (counts.running > 0) {
    const progress = [
      counts.completed > 0
        ? formatTranslation(t, 'agent.read.completedCount', {
            label: getReadCountLabel(t, kind, counts.completed)
          })
        : '',
      counts.pathFailed > 0
        ? formatTranslation(t, 'agent.read.pathFailedCount', { count: counts.pathFailed })
        : '',
      otherFailed > 0 ? formatTranslation(t, 'agent.read.failedCount', { count: otherFailed }) : ''
    ].filter(Boolean)
    return progress.length > 0
      ? `${t(STATUS_LABELS[kind].running)}${t('agent.separator')}${progress.join(t('agent.separator'))}`
      : t(STATUS_LABELS[kind].running)
  }

  if (counts.failed > 0) {
    if (counts.completed > 0 && counts.pathFailed > 0 && otherFailed === 0) {
      return formatTranslation(t, 'agent.read.completedWithPathFailures', {
        label: getReadCountLabel(t, kind, counts.completed),
        count: counts.pathFailed
      })
    }
    if (counts.completed > 0) {
      return [
        formatTranslation(t, 'agent.read.completedCount', {
          label: getReadCountLabel(t, kind, counts.completed)
        }),
        counts.pathFailed > 0
          ? formatTranslation(t, 'agent.read.pathFailedCount', { count: counts.pathFailed })
          : '',
        otherFailed > 0
          ? formatTranslation(t, 'agent.read.failedCount', { count: otherFailed })
          : ''
      ]
        .filter(Boolean)
        .join(t('agent.separator'))
    }
    if (counts.pathFailed === counts.failed) {
      return counts.pathFailed === 1
        ? t('agent.read.file.pathIsDirectory')
        : formatTranslation(t, 'agent.read.pathFailedCount', { count: counts.pathFailed })
    }
    if (counts.pathFailed > 0) {
      return [
        formatTranslation(t, 'agent.read.pathFailedCount', { count: counts.pathFailed }),
        formatTranslation(t, 'agent.read.failedCount', { count: otherFailed })
      ].join(t('agent.separator'))
    }
    if (items.length > 1) {
      return formatTranslation(t, 'agent.read.failedReadCount', {
        label: getReadCountLabel(t, kind, items.length)
      })
    }
    return t(STATUS_LABELS[kind].failed)
  }

  if (counts.cancelled > 0) {
    if (counts.completed > 0) {
      return [
        formatTranslation(t, 'agent.read.completedCount', {
          label: getReadCountLabel(t, kind, counts.completed)
        }),
        formatTranslation(t, 'agent.read.cancelledCount', {
          label: getReadCountLabel(t, kind, counts.cancelled)
        })
      ].join(t('agent.separator'))
    }
    if (items.length > 1) {
      return formatTranslation(t, 'agent.read.cancelledCount', {
        label: getReadCountLabel(t, kind, items.length)
      })
    }
    return t(STATUS_LABELS[kind].cancelled)
  }

  if (items.length > 1) {
    return formatTranslation(t, 'agent.read.completedCount', {
      label: getReadCountLabel(t, kind, items.length)
    })
  }
  return t(STATUS_LABELS[kind].completed)
}

function ReadTextRow({ activity, call, result }: ReadToolActivityProps) {
  const { t } = useFrontendConfig()
  const fileName = getDisplayName(activity, call, t)
  const displayPath = getDisplayPath(activity, call, result) || fileName
  const pathIsDirectory = isPathIsDirectoryFailure(call, result)
  const error = activity?.error ?? result?.error

  return (
    <div
      className="read-activity__text-item"
      data-status={error ? 'failed' : undefined}
      title={error ? `${displayPath}\n${error}` : displayPath}
    >
      {pathIsDirectory
        ? formatTranslation(t, 'agent.read.pathIsDirectoryItem', { path: displayPath })
        : formatTranslation(t, 'agent.read.item', { fileName: displayPath })}
    </div>
  )
}

function ReadActivityCard({
  activity,
  artifactResolver = lazyHostImageArtifactResolver,
  call,
  projectId,
  result
}: ReadToolActivityProps) {
  const { t } = useFrontendConfig()
  const openImagePreview = useImagePreview()
  const showImagePreviewNotice = useImagePreviewNotice()
  const kind = getKind(call, activity)
  const thumbnailDataUrl =
    kind === 'image' ? normalizeReadImageThumbnailDataUrl(activity?.thumbnailDataUrl) : undefined
  const error = activity?.error ?? result?.error
  const displayName = getDisplayName(activity, call, t)
  const sourcePath = activity?.path || getPathFromCall(call)

  const openReadImagePreview = async () => {
    if (!sourcePath) {
      showImagePreviewNotice(t('imagePreview.originalMissing'))
      return
    }

    try {
      const generatedArtifact = generatedArtifactFromResult(sourcePath, result)
      if (generatedArtifact) {
        const resolved = await artifactResolver.resolve(generatedArtifact)
        openResolvedArtifact(generatedArtifact, resolved, openImagePreview)
        return
      }
      const image = await loadImageFile({ projectId, filePath: sourcePath })
      if (!image?.mimeType.startsWith('image/') || !image.data) {
        showImagePreviewNotice(t('imagePreview.originalMissing'))
        return
      }

      openImagePreview({
        alt: image.name || displayName,
        fileName: image.name || displayName,
        src: imageDataUrlFromRecord(image)
      })
    } catch (error) {
      console.error('Failed to load read_image source', error)
      showImagePreviewNotice(t('imagePreview.originalMissing'))
    }
  }

  if (thumbnailDataUrl && !error) {
    return (
      <button
        className="read-activity__image"
        onClick={() => void openReadImagePreview()}
        title={activity?.path || getPathFromCall(call) || getDisplayName(activity, call, t)}
        type="button"
      >
        <img src={thumbnailDataUrl} alt={displayName} />
      </button>
    )
  }

  return <ReadTextRow activity={activity} call={call} result={result} />
}

function ReadActivityDetails({
  activity,
  artifactResolver,
  call,
  projectId,
  result,
  settledStatus
}: ReadToolActivityProps) {
  const error = activity?.error ?? result?.error

  return (
    <div className="agent-activity__details read-activity__details">
      {error ? <p className="read-activity__error">{error}</p> : null}
      <div className="read-activity__items" data-kind={getKind(call, activity)}>
        <ReadActivityCard
          activity={activity}
          artifactResolver={artifactResolver}
          call={call}
          projectId={projectId}
          result={result}
          settledStatus={settledStatus}
        />
      </div>
    </div>
  )
}

export function ReadToolActivity({
  activity,
  artifactResolver,
  call,
  projectId,
  result,
  settledStatus
}: ReadToolActivityProps) {
  const { t } = useFrontendConfig()
  const kind = getKind(call, activity)
  const status = getStatus(activity, result, settledStatus)
  const label = isPathIsDirectoryFailure(call, result)
    ? t('agent.read.file.pathIsDirectory')
    : t(STATUS_LABELS[kind][status])
  const StatusIcon = getStatusIcon(kind)
  const hasDetails = Boolean(activity || getPathFromCall(call) || result?.error)
  const isPending = status === 'running'

  return (
    <AgentActivityDisclosure
      className="agent-activity--read"
      hasDetails={hasDetails}
      icon={StatusIcon}
      isPending={isPending}
      label={label}
    >
      <ReadActivityDetails
        activity={activity}
        artifactResolver={artifactResolver}
        call={call}
        projectId={projectId}
        result={result}
        settledStatus={settledStatus}
      />
    </AgentActivityDisclosure>
  )
}

export function ReadToolActivityGroup({ items, projectId }: ReadToolActivityGroupProps) {
  const { t } = useFrontendConfig()
  const firstItem = items[0]
  if (!firstItem) return null
  if (items.length === 1) {
    return (
      <ReadToolActivity
        activity={firstItem.activity}
        artifactResolver={firstItem.artifactResolver}
        call={firstItem.call}
        projectId={projectId}
        result={firstItem.result}
        settledStatus={firstItem.settledStatus}
      />
    )
  }

  const kind = getKind(firstItem.call, firstItem.activity)
  const label = getReadGroupLabel(t, kind, items)
  const StatusIcon = getStatusIcon(kind)
  const isPending = items.some(
    (item) => getStatus(item.activity, item.result, item.settledStatus) === 'running'
  )
  const hasDetails = items.some((item) =>
    Boolean(item.activity || getPathFromCall(item.call) || item.result?.error)
  )

  return (
    <AgentActivityDisclosure
      className="agent-activity--read"
      hasDetails={hasDetails}
      icon={StatusIcon}
      isPending={isPending}
      label={label}
    >
      <div className="agent-activity__details read-activity__details">
        <div className="read-activity__items" data-kind={kind}>
          {items.map((item) => (
            <ReadActivityCard
              activity={item.activity}
              artifactResolver={item.artifactResolver}
              call={item.call}
              key={item.call.id}
              projectId={projectId}
              result={item.result}
              settledStatus={item.settledStatus}
            />
          ))}
        </div>
      </div>
    </AgentActivityDisclosure>
  )
}
