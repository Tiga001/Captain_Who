// Renderer UI.
import { FileSpreadsheet, FileText, FileType, ImageIcon, Presentation } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import type { ChatReadActivity, ChatReadActivityKind } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface ReadToolActivityProps {
  activity?: ChatReadActivity
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface ReadToolActivityGroupItem extends ReadToolActivityProps {}

interface ReadToolActivityGroupProps {
  items: ReadToolActivityGroupItem[]
}

const TOOL_KINDS: Partial<Record<string, ChatReadActivityKind>> = {
  read_file: 'file',
  read_image: 'image',
  read_pdf: 'pdf',
  read_presentation: 'presentation',
  read_spreadsheet: 'spreadsheet',
  read_word: 'word'
}

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
  pdf: {
    running: 'agent.read.pdf.running',
    completed: 'agent.read.pdf.completed',
    failed: 'agent.read.pdf.failed',
    cancelled: 'agent.read.pdf.cancelled'
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

function getFileName(path: string) {
  const normalized = path.replace(/\\/g, '/').replace(/\/$/, '')
  return normalized.split('/').filter(Boolean).pop() || normalized
}

function getDisplayName(activity: ChatReadActivity | undefined, call: AgentToolCall, t: Translate) {
  if (activity?.fileName) return activity.fileName
  return getFileName(getPathFromCall(call)) || t('agent.read.fallbackFile')
}

function getStatusIcon(kind: ChatReadActivityKind) {
  if (kind === 'image') return ImageIcon
  if (kind === 'spreadsheet') return FileSpreadsheet
  if (kind === 'presentation') return Presentation
  if (kind === 'pdf' || kind === 'word') return FileType
  return FileText
}

function normalizeThumbnailDataUrl(activity: ChatReadActivity | undefined) {
  const thumbnailDataUrl = activity?.thumbnailDataUrl?.trim()
  return thumbnailDataUrl?.startsWith('data:image/') ? thumbnailDataUrl : undefined
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
      return currentCounts
    },
    { cancelled: 0, completed: 0, failed: 0, running: 0 }
  )

  if (counts.running > 0) {
    const progress = [
      counts.completed > 0
        ? formatTranslation(t, 'agent.read.completedCount', {
            label: getReadCountLabel(t, kind, counts.completed)
          })
        : '',
      counts.failed > 0
        ? formatTranslation(t, 'agent.read.failedCount', { count: counts.failed })
        : ''
    ].filter(Boolean)
    return progress.length > 0
      ? `${t(STATUS_LABELS[kind].running)}${t('agent.separator')}${progress.join(t('agent.separator'))}`
      : t(STATUS_LABELS[kind].running)
  }

  if (counts.failed > 0) {
    if (counts.completed > 0) {
      return [
        formatTranslation(t, 'agent.read.completedCount', {
          label: getReadCountLabel(t, kind, counts.completed)
        }),
        formatTranslation(t, 'agent.read.failedCount', { count: counts.failed })
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
  const error = activity?.error ?? result?.error

  return (
    <div
      className="read-activity__text-item"
      data-status={error ? 'failed' : undefined}
      title={
        error
          ? `${activity?.path || getPathFromCall(call) || fileName}\n${error}`
          : activity?.path || getPathFromCall(call) || fileName
      }
    >
      {formatTranslation(t, 'agent.read.item', { fileName })}
    </div>
  )
}

function ReadActivityCard({ activity, call, result }: ReadToolActivityProps) {
  const { t } = useFrontendConfig()
  const kind = getKind(call, activity)
  const thumbnailDataUrl = kind === 'image' ? normalizeThumbnailDataUrl(activity) : undefined
  const error = activity?.error ?? result?.error

  if (thumbnailDataUrl && !error) {
    return (
      <div
        className="read-activity__image"
        title={activity?.path || getPathFromCall(call) || getDisplayName(activity, call, t)}
      >
        <img src={thumbnailDataUrl} alt={getDisplayName(activity, call, t)} />
      </div>
    )
  }

  return <ReadTextRow activity={activity} call={call} result={result} />
}

function ReadActivityDetails({ activity, call, result, settledStatus }: ReadToolActivityProps) {
  const error = activity?.error ?? result?.error

  return (
    <div className="agent-activity__details read-activity__details">
      {error ? <p className="read-activity__error">{error}</p> : null}
      <div className="read-activity__items" data-kind={getKind(call, activity)}>
        <ReadActivityCard
          activity={activity}
          call={call}
          result={result}
          settledStatus={settledStatus}
        />
      </div>
    </div>
  )
}

export function ReadToolActivity({ activity, call, result, settledStatus }: ReadToolActivityProps) {
  const { t } = useFrontendConfig()
  const kind = getKind(call, activity)
  const status = getStatus(activity, result, settledStatus)
  const label = t(STATUS_LABELS[kind][status])
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
        call={call}
        result={result}
        settledStatus={settledStatus}
      />
    </AgentActivityDisclosure>
  )
}

export function ReadToolActivityGroup({ items }: ReadToolActivityGroupProps) {
  const { t } = useFrontendConfig()
  const firstItem = items[0]
  if (!firstItem) return null
  if (items.length === 1) {
    return (
      <ReadToolActivity
        activity={firstItem.activity}
        call={firstItem.call}
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
              call={item.call}
              key={item.call.id}
              result={item.result}
              settledStatus={item.settledStatus}
            />
          ))}
        </div>
      </div>
    </AgentActivityDisclosure>
  )
}
