import { useEffect, useRef, useState, type JSX } from 'react'
import { ChevronDown, ChevronRight, FilePenLine, LoaderCircle } from 'lucide-react'
import type { AgentFileDraftSnapshot, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { getAgentFileWriteDiff, readAgentFileDraft } from '../../../agent/agentClient'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

export interface FileWriteToolActivityGroupItem {
  call: AgentToolCall
  draft?: AgentFileDraftSnapshot
  draftId: string
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

interface FileWriteToolActivityGroupProps {
  items: FileWriteToolActivityGroupItem[]
}

type FileWriteOperation = 'create' | 'update'
type FileWriteStatus =
  'waiting' | 'running' | 'applying' | 'applied' | 'failed' | 'conflict' | 'rejected' | 'stopped'

interface FileWriteItemView {
  additions: number
  deletions: number
  error: string
  filePath: string
  operation: FileWriteOperation
  rejectionReason: string
  status: FileWriteStatus
}

const ROW_LABELS: Record<FileWriteOperation, Record<FileWriteStatus, TranslationKey>> = {
  create: {
    waiting: 'agent.fileWrite.create.row.waiting',
    running: 'agent.fileWrite.create.row.running',
    applying: 'agent.fileWrite.create.row.applying',
    applied: 'agent.fileWrite.create.row.applied',
    failed: 'agent.fileWrite.create.row.failed',
    conflict: 'agent.fileWrite.create.row.conflict',
    rejected: 'agent.fileWrite.create.row.rejected',
    stopped: 'agent.fileWrite.create.row.stopped'
  },
  update: {
    waiting: 'agent.fileWrite.update.row.waiting',
    running: 'agent.fileWrite.update.row.running',
    applying: 'agent.fileWrite.update.row.applying',
    applied: 'agent.fileWrite.update.row.applied',
    failed: 'agent.fileWrite.update.row.failed',
    conflict: 'agent.fileWrite.update.row.conflict',
    rejected: 'agent.fileWrite.update.row.rejected',
    stopped: 'agent.fileWrite.update.row.stopped'
  }
}

function AnimatedInteger({ value }: { value: number }): JSX.Element {
  const [displayed, setDisplayed] = useState(value)
  const previousRef = useRef(value)

  useEffect(() => {
    const from = previousRef.current
    previousRef.current = value
    if (from === value || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setDisplayed(value)
      return
    }
    const startedAt = performance.now()
    const duration = 260
    let frameId = 0
    const update = (now: number): void => {
      const progress = Math.min(1, (now - startedAt) / duration)
      const eased = 1 - Math.pow(1 - progress, 3)
      setDisplayed(Math.round(from + (value - from) * eased))
      if (progress < 1) frameId = window.requestAnimationFrame(update)
    }
    frameId = window.requestAnimationFrame(update)
    return () => window.cancelAnimationFrame(frameId)
  }, [value])

  return <>{displayed}</>
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getString(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function getCallArgs(call: AgentToolCall): Record<string, unknown> {
  return isRecord(call.args) ? call.args : {}
}

function getResultValue(result: AgentToolResult | undefined): Record<string, unknown> {
  return isRecord(result?.result) ? result.result : {}
}

function getStatus(item: FileWriteToolActivityGroupItem): FileWriteStatus {
  const resultStatus = getString(getResultValue(item.result).status)
  if (resultStatus === 'applied' || resultStatus === 'already_applied') return 'applied'
  if (resultStatus === 'failed') return 'failed'
  if (resultStatus === 'conflict') return 'conflict'
  if (resultStatus === 'rejected') return 'rejected'

  switch (item.draft?.status) {
    case 'waiting_approval':
      return 'waiting'
    case 'applying':
      return 'applying'
    case 'applied':
      return 'applied'
    case 'failed':
      return 'failed'
    case 'conflict':
      return 'conflict'
    case 'rejected':
      return 'rejected'
    case 'aborted':
    case 'expired':
      return 'stopped'
    default:
      break
  }

  if (item.result?.ok === false) return 'failed'
  if (item.call.approvalStatus === 'required') return 'waiting'
  if (item.call.approvalStatus === 'rejected') return 'rejected'
  if (item.settledStatus === 'failed') return 'failed'
  if (item.settledStatus === 'cancelled') return 'stopped'
  if (item.settledStatus === 'completed') return 'applied'
  return 'running'
}

function getOperation(item: FileWriteToolActivityGroupItem): FileWriteOperation {
  const args = getCallArgs(item.call)
  const mode = item.draft?.mode ?? getString(args.mode)
  if (mode === 'create') return 'create'
  if (mode === 'upsert' && !item.draft?.baseRevision) return 'create'
  return 'update'
}

function getItemView(item: FileWriteToolActivityGroupItem): FileWriteItemView {
  const args = getCallArgs(item.call)
  const result = getResultValue(item.result)
  const status = getStatus(item)
  const error = getString(result.error) || item.result?.error || ''
  const message = getString(result.message)

  return {
    additions: item.draft?.additions ?? 0,
    deletions: item.draft?.deletions ?? 0,
    error: status === 'failed' || status === 'conflict' ? error || message : '',
    filePath: item.draft?.filePath ?? getString(args.filePath),
    operation: getOperation(item),
    rejectionReason: status === 'rejected' ? message || error : '',
    status
  }
}

function isPending(status: FileWriteStatus): boolean {
  return status === 'waiting' || status === 'running' || status === 'applying'
}

function getGroupLabel(items: FileWriteToolActivityGroupItem[], t: Translate): string {
  const counts = items.reduce(
    (current, item) => {
      current[getStatus(item)] += 1
      return current
    },
    {
      applied: 0,
      applying: 0,
      conflict: 0,
      failed: 0,
      rejected: 0,
      running: 0,
      stopped: 0,
      waiting: 0
    }
  )
  const count = String(items.length)

  if (counts.waiting > 0) {
    return formatTranslation(t, 'agent.fileWrite.group.waiting', { count })
  }
  if (counts.running > 0 || counts.applying > 0) {
    return formatTranslation(t, 'agent.fileWrite.group.running', { count })
  }
  if (
    counts.failed === 0 &&
    counts.conflict === 0 &&
    counts.rejected === 0 &&
    counts.stopped === 0
  ) {
    return formatTranslation(t, 'agent.fileWrite.group.applied', { count })
  }

  const parts = [
    counts.applied > 0
      ? formatTranslation(t, 'agent.fileWrite.group.appliedCount', {
          count: String(counts.applied)
        })
      : '',
    counts.failed > 0
      ? formatTranslation(t, 'agent.fileWrite.group.failedCount', { count: String(counts.failed) })
      : '',
    counts.conflict > 0
      ? formatTranslation(t, 'agent.fileWrite.group.conflictCount', {
          count: String(counts.conflict)
        })
      : '',
    counts.rejected > 0
      ? formatTranslation(t, 'agent.fileWrite.group.rejectedCount', {
          count: String(counts.rejected)
        })
      : '',
    counts.stopped > 0
      ? formatTranslation(t, 'agent.fileWrite.group.stoppedCount', {
          count: String(counts.stopped)
        })
      : ''
  ].filter(Boolean)

  return [formatTranslation(t, 'agent.fileWrite.group.processed', { count }), ...parts].join(
    t('agent.separator')
  )
}

function FileWriteEntry({
  item,
  standalone
}: {
  item: FileWriteToolActivityGroupItem
  standalone: boolean
}): JSX.Element {
  const { t } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const [preview, setPreview] = useState('')
  const [previewError, setPreviewError] = useState('')
  const [loading, setLoading] = useState(false)
  const view = getItemView(item)
  const canPreview = Boolean(item.draft)

  useEffect(() => {
    if (!expanded || !item.draft) return
    let cancelled = false
    const shouldShowDiff = item.draft.statsFinal || item.draft.status === 'waiting_approval'
    const request = shouldShowDiff
      ? getAgentFileWriteDiff(item.draftId).then((page) => page.patch)
      : readAgentFileDraft(item.draftId).then((page) => page.content)
    void request
      .then((content) => {
        if (!cancelled) setPreview(content)
      })
      .catch((error) => {
        if (!cancelled) setPreviewError(error instanceof Error ? error.message : String(error))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [expanded, item.draft, item.draftId])

  const toggleExpanded = (): void => {
    if (!expanded) {
      setLoading(true)
      setPreviewError('')
    }
    setExpanded((value) => !value)
  }

  const line = (
    <div className={standalone ? 'file-write-activity__summary' : 'file-write-activity__item-line'}>
      {standalone ? (
        <span className="file-write-activity__icon" aria-hidden="true">
          {view.status === 'running' || view.status === 'applying' ? (
            <LoaderCircle className="file-write-activity__spinner" />
          ) : (
            <FilePenLine />
          )}
        </span>
      ) : null}
      <span className={isPending(view.status) ? 'agent-running-text' : undefined}>
        {t(ROW_LABELS[view.operation][view.status])}
      </span>
      <span className="file-write-activity__path" title={view.filePath}>
        {view.filePath || t('agent.fileWrite.unknownFile')}
      </span>
      <span
        className="file-write-activity__stats"
        aria-label={`+${view.additions} -${view.deletions}`}
      >
        <span className="file-write-activity__additions">
          +<AnimatedInteger value={view.additions} />
        </span>
        <span className="file-write-activity__deletions">
          -<AnimatedInteger value={view.deletions} />
        </span>
      </span>
      {canPreview ? (
        <button
          aria-expanded={expanded}
          aria-label={t('agent.fileWrite.togglePreview')}
          className="file-write-activity__toggle"
          onClick={toggleExpanded}
          title={t('agent.fileWrite.togglePreview')}
          type="button"
        >
          {expanded ? <ChevronDown /> : <ChevronRight />}
        </button>
      ) : null}
    </div>
  )

  return (
    <div
      className={
        standalone ? 'agent-activity agent-activity--file-write' : 'file-write-activity__item'
      }
    >
      {line}
      {view.rejectionReason ? (
        <p className="file-write-activity__rejection-reason">{view.rejectionReason}</p>
      ) : null}
      {view.error ? <p className="file-write-activity__error-note">{view.error}</p> : null}
      {expanded ? (
        <div className="file-write-activity__preview">
          {loading ? <span>{t('agent.fileWrite.loadingPreview')}</span> : null}
          {previewError ? <span className="file-write-activity__error">{previewError}</span> : null}
          {!loading && !previewError ? <pre>{preview}</pre> : null}
        </div>
      ) : null}
    </div>
  )
}

export function FileWriteToolActivityGroup({
  items
}: FileWriteToolActivityGroupProps): JSX.Element | null {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  if (items.length === 1) return <FileWriteEntry item={items[0]} standalone />

  return (
    <AgentActivityDisclosure
      className="agent-activity--file-write"
      hasDetails
      icon={FilePenLine}
      isPending={items.some((item) => isPending(getStatus(item)))}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details file-write-activity__details">
        {items.map((item) => (
          <FileWriteEntry item={item} key={item.draftId} standalone={false} />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
