import { ChevronDown, ChevronRight, Pencil } from 'lucide-react'
import { useEffect, useState } from 'react'
import type {
  AgentFileChangeOperation,
  AgentFileChangeProposal,
  AgentFileChangeResult,
  AgentFileChangeSnapshot,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import { getAgentFileChangeDiff, readAgentFileChange } from '../../../agent/agentClient'
import { getApplyPatchRequest } from '../../../agentRun/applyPatchRequest'
import { revealStoredProjectFile } from '../../../storage/storageClient'
import type { ChatFileChangePreview } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { getSafeFileChangeFailureMessage } from './fileChangeFailurePresentation'
import type { SettledToolStatus } from './toolActivityUtils'

const PREVIEW_PAGE_CHARS = 50_000

export interface FileChangeToolActivityGroupItem {
  cancelled?: boolean
  call: AgentToolCall
  preview?: ChatFileChangePreview
  projectId?: string | null
  proposal?: AgentFileChangeProposal
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
  transaction?: AgentFileChangeSnapshot
  transactionId?: string
}

interface FileChangeToolActivityGroupProps {
  items: FileChangeToolActivityGroupItem[]
  observerRootConversationId?: string
  projectId?: string | null
}

interface FileChangeToolActivityProps extends Omit<
  FileChangeToolActivityGroupItem,
  'transactionId'
> {
  observerRootConversationId?: string
  transactionId?: string
}

export type FileChangeStatus =
  | 'waiting'
  | 'running'
  | 'applied'
  | 'failed'
  | 'conflict'
  | 'rejected'
  | 'cancelled'
  | 'outcome_unknown'

const ROW_LABELS: Record<AgentFileChangeOperation, Record<FileChangeStatus, TranslationKey>> = {
  create: {
    waiting: 'agent.fileChange.create.row.waiting',
    running: 'agent.fileChange.create.row.running',
    applied: 'agent.fileChange.create.row.applied',
    failed: 'agent.fileChange.create.row.failed',
    conflict: 'agent.fileChange.create.row.conflict',
    rejected: 'agent.fileChange.create.row.rejected',
    cancelled: 'agent.fileChange.create.row.cancelled',
    outcome_unknown: 'agent.fileChange.failure.outcomeUnknown'
  },
  update: {
    waiting: 'agent.fileChange.update.row.waiting',
    running: 'agent.fileChange.update.row.running',
    applied: 'agent.fileChange.update.row.applied',
    failed: 'agent.fileChange.update.row.failed',
    conflict: 'agent.fileChange.update.row.conflict',
    rejected: 'agent.fileChange.update.row.rejected',
    cancelled: 'agent.fileChange.update.row.cancelled',
    outcome_unknown: 'agent.fileChange.failure.outcomeUnknown'
  },
  delete: {
    waiting: 'agent.fileChange.delete.row.waiting',
    running: 'agent.fileChange.delete.row.running',
    applied: 'agent.fileChange.delete.row.applied',
    failed: 'agent.fileChange.delete.row.failed',
    conflict: 'agent.fileChange.delete.row.conflict',
    rejected: 'agent.fileChange.delete.row.rejected',
    cancelled: 'agent.fileChange.delete.row.cancelled',
    outcome_unknown: 'agent.fileChange.failure.outcomeUnknown'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getString(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function getRawString(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function getNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? Math.max(0, value) : undefined
}

function getOperation(value: unknown): AgentFileChangeOperation | undefined {
  return value === 'create' || value === 'update' || value === 'delete' ? value : undefined
}

function getCallArgs(call: AgentToolCall): Record<string, unknown> {
  return getApplyPatchRequest(call.args) ?? {}
}

function getFileChangeResult(
  result: AgentToolResult | undefined
): AgentFileChangeResult | undefined {
  if (!isRecord(result?.result)) return undefined
  const status = result.result.status
  const operation = getOperation(result.result.operation)
  if (
    result.result.schemaVersion !== 1 ||
    typeof result.result.transactionId !== 'string' ||
    !getString(result.result.filePath) ||
    !operation ||
    ![
      'applied',
      'already_applied',
      'failed',
      'conflict',
      'rejected',
      'outcome_unknown',
      'aborted',
      'expired'
    ].includes(String(status))
  ) {
    return undefined
  }
  return result.result as unknown as AgentFileChangeResult
}

function getStatus(item: FileChangeToolActivityGroupItem): FileChangeStatus {
  const result = getFileChangeResult(item.result)
  if (result?.status === 'applied' || result?.status === 'already_applied') return 'applied'
  if (result?.status === 'failed') return 'failed'
  if (result?.status === 'conflict') return 'conflict'
  if (result?.status === 'rejected') return 'rejected'
  if (result?.status === 'outcome_unknown') return 'outcome_unknown'
  if (result?.status === 'aborted' || result?.status === 'expired') return 'cancelled'

  // A terminal Run settles any non-terminal transaction projection. The durable transaction
  // snapshot can lag the Run terminal event after cancellation or failure; it must not keep the
  // activity looking actionable or in progress while recovery retires the transaction.
  if (
    item.transaction &&
    ['drafting', 'ready', 'waiting_approval', 'applying'].includes(item.transaction.status)
  ) {
    if (item.cancelled || item.settledStatus === 'cancelled') return 'cancelled'
    if (item.settledStatus === 'failed') return 'failed'
  }

  switch (item.transaction?.status) {
    case 'waiting_approval':
      return 'waiting'
    case 'applying':
    case 'drafting':
    case 'ready':
      return 'running'
    case 'applied':
    case 'already_applied':
      return 'applied'
    case 'failed':
      return 'failed'
    case 'conflict':
      return 'conflict'
    case 'rejected':
      return 'rejected'
    case 'outcome_unknown':
      return 'outcome_unknown'
    case 'aborted':
    case 'expired':
      return 'cancelled'
    default:
      break
  }

  if (item.result?.ok === false) return 'failed'
  if (item.call.approvalStatus === 'rejected') return 'rejected'
  if (item.call.approvalStatus === 'required') return 'waiting'
  if (item.cancelled || item.settledStatus === 'cancelled') return 'cancelled'
  if (item.settledStatus === 'failed') return 'failed'
  if (item.settledStatus === 'completed') return 'applied'
  return 'running'
}

function getPreviewTransactionId(item: FileChangeToolActivityGroupItem): string | undefined {
  const result = getFileChangeResult(item.result)
  const requestTransactionId = getString(getCallArgs(item.call).transactionId)
  return (
    item.transactionId ||
    item.preview?.transactionId ||
    item.proposal?.transactionId ||
    item.transaction?.transactionId ||
    result?.transactionId ||
    requestTransactionId ||
    undefined
  )
}

function countPatchLines(patch: string) {
  return patch.split(/\r?\n/).reduce(
    (counts, line) => {
      if (line.startsWith('+++ ') || line.startsWith('--- ')) return counts
      if (line.startsWith('+')) counts.additions += 1
      if (line.startsWith('-')) counts.deletions += 1
      return counts
    },
    { additions: 0, deletions: 0 }
  )
}

function countTextLines(content: string): number {
  const normalized = content.replace(/\r\n/g, '\n').replace(/\n$/, '')
  return normalized ? normalized.split('\n').length : 0
}

function countStructuredEditLines(edits: unknown) {
  if (!Array.isArray(edits)) return undefined
  return edits.reduce(
    (counts, edit) => {
      if (!isRecord(edit)) return counts
      const kind = getString(edit.kind)
      if (kind === 'replace') {
        counts.additions += countTextLines(getRawString(edit.newText))
        counts.deletions += countTextLines(getRawString(edit.oldText))
      } else if (['insert_before', 'insert_after', 'append', 'prepend'].includes(kind)) {
        counts.additions += countTextLines(getRawString(edit.text))
      }
      return counts
    },
    { additions: 0, deletions: 0 }
  )
}

function getFallbackLineCounts(args: Record<string, unknown>, operation: AgentFileChangeOperation) {
  const editCounts = countStructuredEditLines(args.edits)
  if (editCounts) return editCounts
  const content = getRawString(args.content)
  return content && operation === 'create'
    ? { additions: countTextLines(content), deletions: 0 }
    : { additions: 0, deletions: 0 }
}

export function getFileChangeItemView(item: FileChangeToolActivityGroupItem) {
  const args = getCallArgs(item.call)
  const result = getFileChangeResult(item.result)
  const resultValue = isRecord(item.result?.result) ? item.result.result : {}
  const operation =
    result?.operation ??
    item.proposal?.operation ??
    item.transaction?.operation ??
    getOperation(args.operation) ??
    'update'
  const inlinePatch = item.proposal?.inlineDiff?.patch ?? ''
  const fallbackCounts = inlinePatch
    ? countPatchLines(inlinePatch)
    : getFallbackLineCounts(args, operation)
  const status = getStatus(item)
  const preview = status === 'running' ? item.preview : undefined
  return {
    additions:
      getNumber(resultValue.additions) ??
      preview?.additions ??
      item.proposal?.additions ??
      item.transaction?.additions ??
      fallbackCounts.additions,
    deletions:
      getNumber(resultValue.deletions) ??
      preview?.deletions ??
      item.proposal?.deletions ??
      item.transaction?.deletions ??
      fallbackCounts.deletions,
    filePath:
      preview?.filePath ??
      result?.filePath ??
      item.proposal?.filePath ??
      item.transaction?.filePath ??
      getString(args.filePath),
    operation,
    rejectionReason: status === 'rejected' ? result?.message?.trim() || '' : '',
    status
  }
}

function isPending(status: FileChangeStatus): boolean {
  return status === 'waiting' || status === 'running'
}

function getGroupLabel(items: FileChangeToolActivityGroupItem[], t: Translate): string {
  const counts = items.reduce(
    (current, item) => {
      current[getStatus(item)] += 1
      return current
    },
    {
      applied: 0,
      cancelled: 0,
      conflict: 0,
      failed: 0,
      outcome_unknown: 0,
      rejected: 0,
      running: 0,
      waiting: 0
    }
  )
  const count = String(items.length)
  if (counts.waiting > 0) return formatTranslation(t, 'agent.fileChange.group.waiting', { count })
  if (counts.running > 0) return formatTranslation(t, 'agent.fileChange.group.running', { count })
  if (
    counts.failed === 0 &&
    counts.conflict === 0 &&
    counts.rejected === 0 &&
    counts.cancelled === 0 &&
    counts.outcome_unknown === 0
  ) {
    return formatTranslation(t, 'agent.fileChange.group.applied', { count })
  }
  const parts = [
    counts.applied
      ? formatTranslation(t, 'agent.fileChange.group.appliedCount', {
          count: String(counts.applied)
        })
      : '',
    counts.failed
      ? formatTranslation(t, 'agent.fileChange.group.failedCount', {
          count: String(counts.failed)
        })
      : '',
    counts.conflict
      ? formatTranslation(t, 'agent.fileChange.group.conflictCount', {
          count: String(counts.conflict)
        })
      : '',
    counts.rejected
      ? formatTranslation(t, 'agent.fileChange.group.rejectedCount', {
          count: String(counts.rejected)
        })
      : '',
    counts.cancelled
      ? formatTranslation(t, 'agent.fileChange.group.cancelledCount', {
          count: String(counts.cancelled)
        })
      : '',
    counts.outcome_unknown ? t('agent.fileChange.failure.outcomeUnknown') : ''
  ].filter(Boolean)
  return [formatTranslation(t, 'agent.fileChange.group.processed', { count }), ...parts].join(
    t('agent.separator')
  )
}

function isAbsoluteLocalPath(filePath: string): boolean {
  return filePath.startsWith('/') || /^[A-Za-z]:[\\/]/.test(filePath) || filePath.startsWith('\\\\')
}

function FileChangeRow({
  item,
  observerRootConversationId,
  projectId
}: {
  item: FileChangeToolActivityGroupItem
  observerRootConversationId?: string
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const [persistedPreview, setPersistedPreview] = useState('')
  const [previewError, setPreviewError] = useState('')
  const [loading, setLoading] = useState(false)
  const [nextOffset, setNextOffset] = useState<number | null>(null)
  const view = getFileChangeItemView(item)
  const structuredResult = getFileChangeResult(item.result)
  const inlinePatch = item.proposal?.inlineDiff?.patch ?? null
  const previewTransactionId = getPreviewTransactionId(item)
  const previewSource =
    item.proposal ||
    structuredResult ||
    item.transaction?.statsFinal ||
    item.transaction?.status === 'waiting_approval'
      ? 'diff'
      : 'content'
  const hasLivePreview = view.status === 'running' && Boolean(item.preview)
  const displayedPreview = hasLivePreview
    ? `${persistedPreview}${item.preview?.content ?? ''}`
    : persistedPreview
  const canPreview = Boolean(inlinePatch !== null || item.preview || previewTransactionId)
  const safePreviewError = t('files.preview.error')
  const canReveal = Boolean(view.filePath && (projectId || isAbsoluteLocalPath(view.filePath)))

  useEffect(() => {
    if (!expanded || !canPreview) return
    let cancelled = false
    setPersistedPreview('')
    setNextOffset(null)
    setPreviewError('')
    if (inlinePatch !== null) {
      setPersistedPreview(inlinePatch)
      setLoading(false)
      return
    }
    if (!previewTransactionId) {
      setLoading(false)
      return
    }
    setLoading(true)
    const request =
      previewSource === 'diff'
        ? getAgentFileChangeDiff(
            previewTransactionId,
            0,
            PREVIEW_PAGE_CHARS,
            observerRootConversationId
          ).then((page) => {
            if (page.transactionId !== previewTransactionId || page.offset !== 0) {
              throw new Error('FileChange preview identity mismatch')
            }
            return { content: page.patch, nextOffset: page.nextOffset }
          })
        : readAgentFileChange(
            previewTransactionId,
            0,
            PREVIEW_PAGE_CHARS,
            observerRootConversationId
          ).then((page) => {
            if (page.fileChange.transactionId !== previewTransactionId || page.offset !== 0) {
              throw new Error('FileChange preview identity mismatch')
            }
            return { content: page.content, nextOffset: page.nextOffset }
          })
    void request
      .then((page) => {
        if (cancelled) return
        setPersistedPreview(page.content)
        setNextOffset(page.nextOffset)
      })
      .catch(() => {
        if (!cancelled) setPreviewError(safePreviewError)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [
    canPreview,
    expanded,
    inlinePatch,
    observerRootConversationId,
    previewTransactionId,
    previewSource,
    safePreviewError
  ])

  const loadMore = (): void => {
    if (nextOffset === null || loading || !previewTransactionId) return
    const requestedOffset = nextOffset
    setLoading(true)
    setPreviewError('')
    const request =
      previewSource === 'diff'
        ? getAgentFileChangeDiff(
            previewTransactionId,
            requestedOffset,
            PREVIEW_PAGE_CHARS,
            observerRootConversationId
          ).then((page) => {
            if (page.transactionId !== previewTransactionId || page.offset !== requestedOffset) {
              throw new Error('FileChange preview identity mismatch')
            }
            return { content: page.patch, nextOffset: page.nextOffset }
          })
        : readAgentFileChange(
            previewTransactionId,
            requestedOffset,
            PREVIEW_PAGE_CHARS,
            observerRootConversationId
          ).then((page) => {
            if (
              page.fileChange.transactionId !== previewTransactionId ||
              page.offset !== requestedOffset
            ) {
              throw new Error('FileChange preview identity mismatch')
            }
            return { content: page.content, nextOffset: page.nextOffset }
          })
    void request
      .then((page) => {
        setPersistedPreview((current) => `${current}${page.content}`)
        setNextOffset(page.nextOffset)
      })
      .catch(() => setPreviewError(safePreviewError))
      .finally(() => setLoading(false))
  }

  const failureNote =
    view.status === 'failed' || view.status === 'conflict' || view.status === 'outcome_unknown'
      ? getSafeFileChangeFailureMessage(view.status, item.result, t)
      : ''

  return (
    <div className="file-change-activity__item">
      <div className="file-change-activity__item-line" title={view.filePath}>
        <span className={isPending(view.status) ? 'agent-running-text' : undefined}>
          {t(ROW_LABELS[view.operation][view.status])}
        </span>
        {canReveal ? (
          <button
            aria-label={formatTranslation(t, 'agent.fileChange.revealFile', {
              filePath: view.filePath
            })}
            className="file-change-activity__path"
            onClick={() => {
              void revealStoredProjectFile(projectId, view.filePath).catch(() => undefined)
            }}
            title={formatTranslation(t, 'agent.fileChange.revealFile', {
              filePath: view.filePath
            })}
            type="button"
          >
            {view.filePath}
          </button>
        ) : (
          <span className="file-change-activity__path">
            {view.filePath || t('agent.fileChange.unknownFile')}
          </span>
        )}
        <span
          className="file-change-activity__stats"
          aria-label={`+${view.additions} -${view.deletions}`}
        >
          <span className="file-change-activity__additions">+{view.additions}</span>
          <span className="file-change-activity__deletions">-{view.deletions}</span>
        </span>
        {canPreview ? (
          <button
            aria-expanded={expanded}
            aria-label={t('agent.fileChange.togglePreview')}
            className="file-change-activity__toggle"
            onClick={() => setExpanded((current) => !current)}
            title={t('agent.fileChange.togglePreview')}
            type="button"
          >
            {expanded ? <ChevronDown /> : <ChevronRight />}
          </button>
        ) : null}
      </div>
      {view.rejectionReason ? (
        <p className="file-change-activity__rejection-reason">{view.rejectionReason}</p>
      ) : null}
      {failureNote ? <p className="file-change-activity__error-note">{failureNote}</p> : null}
      {expanded ? (
        <div className="file-change-activity__preview">
          {loading && !persistedPreview ? (
            <span>{t('agent.fileChange.loadingPreview')}</span>
          ) : null}
          {previewError ? (
            <span className="file-change-activity__error">{previewError}</span>
          ) : null}
          {!previewError && displayedPreview ? <pre>{displayedPreview}</pre> : null}
          {nextOffset !== null && !previewError ? (
            <button disabled={loading} onClick={loadMore} type="button">
              {loading
                ? t('agent.fileChange.loadingPreview')
                : t('agent.fileChange.loadMorePreview')}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

export function FileChangeToolActivity({
  observerRootConversationId,
  transactionId,
  ...item
}: FileChangeToolActivityProps) {
  return (
    <FileChangeToolActivityGroup
      items={[{ ...item, transactionId }]}
      observerRootConversationId={observerRootConversationId}
      projectId={item.projectId}
    />
  )
}

export function FileChangeToolActivityGroup({
  items,
  observerRootConversationId,
  projectId
}: FileChangeToolActivityGroupProps) {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  return (
    <AgentActivityDisclosure
      className="agent-activity--file-change"
      hasDetails
      icon={Pencil}
      isPending={items.some((item) => isPending(getStatus(item)))}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details file-change-activity__details">
        {items.map((item) => (
          <FileChangeRow
            item={item}
            key={item.call.id}
            observerRootConversationId={observerRootConversationId}
            projectId={projectId}
          />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
