import { ChevronDown, Pencil } from 'lucide-react'
import { useState } from 'react'
import type {
  AgentFileChangeOperation,
  AgentFileChangeProposal,
  AgentFileChangeResult,
  AgentToolCall,
  AgentToolResult
} from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { revealStoredProjectFile } from '../../../storage/storageClient'
import type { ChatFileWritePreview } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { getSafeApplyPatchFailureMessage } from './applyPatchFailurePresentation'
import type { SettledToolStatus } from './toolActivityUtils'

interface ApplyPatchToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  diff?: AgentFileChangeProposal
  preview?: ChatFileWritePreview
  projectId?: string | null
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface ApplyPatchToolActivityGroupItem extends ApplyPatchToolActivityProps {}

interface ApplyPatchToolActivityGroupProps {
  items: ApplyPatchToolActivityGroupItem[]
  projectId?: string | null
}

export type ApplyPatchStatus =
  'waiting' | 'running' | 'applied' | 'failed' | 'conflict' | 'rejected' | 'cancelled'

const ROW_LABELS: Record<AgentFileChangeOperation, Record<ApplyPatchStatus, TranslationKey>> = {
  create: {
    waiting: 'agent.patch.create.row.waiting',
    running: 'agent.patch.create.row.running',
    applied: 'agent.patch.create.row.applied',
    failed: 'agent.patch.create.row.failed',
    conflict: 'agent.patch.create.row.conflict',
    rejected: 'agent.patch.create.row.rejected',
    cancelled: 'agent.patch.create.row.cancelled'
  },
  update: {
    waiting: 'agent.patch.update.row.waiting',
    running: 'agent.patch.update.row.running',
    applied: 'agent.patch.update.row.applied',
    failed: 'agent.patch.update.row.failed',
    conflict: 'agent.patch.update.row.conflict',
    rejected: 'agent.patch.update.row.rejected',
    cancelled: 'agent.patch.update.row.cancelled'
  },
  delete: {
    waiting: 'agent.patch.delete.row.waiting',
    running: 'agent.patch.delete.row.running',
    applied: 'agent.patch.delete.row.applied',
    failed: 'agent.patch.delete.row.failed',
    conflict: 'agent.patch.delete.row.conflict',
    rejected: 'agent.patch.delete.row.rejected',
    cancelled: 'agent.patch.delete.row.cancelled'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getString(value: unknown) {
  return typeof value === 'string' ? value.trim() : ''
}

function getRawString(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function getNumber(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? Math.max(0, value) : undefined
}

function getOperation(value: unknown): AgentFileChangeOperation | undefined {
  return value === 'create' || value === 'update' || value === 'delete' ? value : undefined
}

export function isAbsoluteLocalPath(filePath: string) {
  return filePath.startsWith('/') || /^[A-Za-z]:[\\/]/.test(filePath) || filePath.startsWith('\\\\')
}

function getFileChangeResult(
  result: AgentToolResult | undefined
): AgentFileChangeResult | undefined {
  if (!isRecord(result?.result)) return undefined
  const status = result.result.status
  const operation = getOperation(result.result.operation)
  const filePath = getString(result.result.filePath)
  if (
    (status !== 'applied' &&
      status !== 'already_applied' &&
      status !== 'failed' &&
      status !== 'conflict' &&
      status !== 'rejected' &&
      status !== 'outcome_unknown' &&
      status !== 'aborted' &&
      status !== 'expired') ||
    !operation ||
    !filePath ||
    result.result.schemaVersion !== 1 ||
    typeof result.result.transactionId !== 'string'
  ) {
    return undefined
  }

  return result.result as unknown as AgentFileChangeResult
}

function getCallArgs(call: AgentToolCall) {
  return isRecord(call.args) ? call.args : {}
}

function getStatus(item: ApplyPatchToolActivityGroupItem): ApplyPatchStatus {
  const fileChangeResult = getFileChangeResult(item.result)
  if (fileChangeResult?.status === 'applied' || fileChangeResult?.status === 'already_applied')
    return 'applied'
  if (fileChangeResult?.status === 'failed') return 'failed'
  if (fileChangeResult?.status === 'conflict') return 'conflict'
  if (fileChangeResult?.status === 'rejected') return 'rejected'
  if (
    fileChangeResult?.status === 'outcome_unknown' ||
    fileChangeResult?.status === 'aborted' ||
    fileChangeResult?.status === 'expired'
  )
    return 'failed'
  if (item.result?.ok === false) return 'failed'
  if (item.call.approvalStatus === 'rejected') return 'rejected'
  if (item.cancelled) return 'cancelled'
  if (item.call.approvalStatus === 'required') return 'waiting'
  if (item.settledStatus === 'completed') return 'applied'
  if (item.settledStatus === 'failed') return 'failed'
  if (item.settledStatus === 'cancelled') return 'cancelled'
  return 'running'
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

function countTextLines(content: string) {
  const normalized = content.replace(/\r\n/g, '\n').replace(/\n$/, '')
  if (!normalized) return 0
  return normalized.split('\n').length
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
        return counts
      }

      if (
        kind === 'insert_before' ||
        kind === 'insert_after' ||
        kind === 'append' ||
        kind === 'prepend'
      ) {
        counts.additions += countTextLines(getRawString(edit.text))
      }

      return counts
    },
    { additions: 0, deletions: 0 }
  )
}

function getFallbackLineCounts(args: Record<string, unknown>, operation: AgentFileChangeOperation) {
  const structuredCounts = countStructuredEditLines(args.edits)
  if (structuredCounts) return structuredCounts

  const content = getRawString(args.content)
  if (content && operation === 'create') {
    return { additions: countTextLines(content), deletions: 0 }
  }

  return { additions: 0, deletions: 0 }
}

export function getApplyPatchItemView(item: ApplyPatchToolActivityGroupItem) {
  const args = getCallArgs(item.call)
  const fileChangeResult = getFileChangeResult(item.result)
  const resultValue = isRecord(item.result?.result) ? item.result.result : {}
  const operation =
    fileChangeResult?.operation ?? item.diff?.operation ?? getOperation(args.operation) ?? 'update'
  const filePath = fileChangeResult?.filePath ?? item.diff?.filePath ?? getString(args.filePath)
  const patch = item.diff?.inlineDiff?.patch ?? ''
  const parsedCounts = patch ? countPatchLines(patch) : getFallbackLineCounts(args, operation)
  const preview = getStatus(item) === 'running' ? item.preview : undefined
  const additions =
    getNumber(resultValue.additions) ??
    preview?.additions ??
    getNumber(args.additions) ??
    parsedCounts.additions
  const deletions =
    getNumber(resultValue.deletions) ??
    preview?.deletions ??
    getNumber(args.deletions) ??
    parsedCounts.deletions

  return {
    additions,
    deletions,
    filePath: preview?.filePath ?? filePath,
    message: fileChangeResult?.message ?? '',
    operation,
    status: getStatus(item)
  }
}

function getGroupLabel(
  items: ApplyPatchToolActivityGroupItem[],
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const counts = items.reduce(
    (currentCounts, item) => {
      currentCounts[getStatus(item)] += 1
      return currentCounts
    },
    { applied: 0, cancelled: 0, conflict: 0, failed: 0, rejected: 0, running: 0, waiting: 0 }
  )
  const count = String(items.length)

  if (counts.waiting > 0) {
    return formatTranslation(t, 'agent.patch.group.waiting', { count })
  }
  if (counts.running > 0) {
    return formatTranslation(t, 'agent.patch.group.running', { count })
  }
  if (
    counts.failed === 0 &&
    counts.conflict === 0 &&
    counts.rejected === 0 &&
    counts.cancelled === 0
  ) {
    return formatTranslation(t, 'agent.patch.group.applied', { count })
  }

  const parts = [
    counts.applied > 0
      ? formatTranslation(t, 'agent.patch.group.appliedCount', { count: String(counts.applied) })
      : '',
    counts.failed > 0
      ? formatTranslation(t, 'agent.patch.group.failedCount', { count: String(counts.failed) })
      : '',
    counts.conflict > 0
      ? formatTranslation(t, 'agent.patch.group.conflictCount', { count: String(counts.conflict) })
      : '',
    counts.rejected > 0
      ? formatTranslation(t, 'agent.patch.group.rejectedCount', { count: String(counts.rejected) })
      : '',
    counts.cancelled > 0
      ? formatTranslation(t, 'agent.patch.group.cancelledCount', {
          count: String(counts.cancelled)
        })
      : ''
  ].filter(Boolean)

  return [formatTranslation(t, 'agent.patch.group.processed', { count }), ...parts].join(
    t('agent.separator')
  )
}

function ApplyPatchFileRow({
  item,
  projectId
}: {
  item: ApplyPatchToolActivityGroupItem
  projectId?: string | null
}) {
  const { t } = useFrontendConfig()
  const [isRejectionReasonOpen, setIsRejectionReasonOpen] = useState(false)
  const view = getApplyPatchItemView(item)
  const note =
    view.status === 'failed' || view.status === 'conflict'
      ? getSafeApplyPatchFailureMessage(view.status, item.result, t)
      : ''
  const rejectionReason = view.status === 'rejected' ? view.message.trim() : ''
  const showLineStats = view.status !== 'rejected'
  const canReveal = Boolean(view.filePath && (projectId || isAbsoluteLocalPath(view.filePath)))

  return (
    <div className="apply-patch-activity__item">
      <div className="apply-patch-activity__item-line" title={view.filePath}>
        <span>{t(ROW_LABELS[view.operation][view.status])}</span>
        {canReveal ? (
          <button
            aria-label={formatTranslation(t, 'agent.patch.revealFile', {
              filePath: view.filePath
            })}
            className="apply-patch-activity__path"
            onClick={() => {
              void revealStoredProjectFile(projectId, view.filePath).catch((error) => {
                console.error('Failed to reveal patched file', error)
              })
            }}
            title={formatTranslation(t, 'agent.patch.revealFile', {
              filePath: view.filePath
            })}
            type="button"
          >
            {view.filePath}
          </button>
        ) : (
          <span className="apply-patch-activity__path">{view.filePath}</span>
        )}
        {rejectionReason ? (
          <button
            aria-expanded={isRejectionReasonOpen}
            aria-label={t('agent.patch.rejectionReason')}
            className="apply-patch-activity__reason-toggle"
            onClick={() => setIsRejectionReasonOpen((isOpen) => !isOpen)}
            title={t('agent.patch.rejectionReason')}
            type="button"
          >
            <ChevronDown aria-hidden="true" />
          </button>
        ) : null}
        {showLineStats ? (
          <>
            <span className="apply-patch-activity__additions">+{view.additions}</span>
            <span className="apply-patch-activity__deletions">-{view.deletions}</span>
          </>
        ) : null}
      </div>
      {isRejectionReasonOpen && rejectionReason ? (
        <p className="apply-patch-activity__rejection-reason">{rejectionReason}</p>
      ) : null}
      {note ? <p className="apply-patch-activity__note">{note}</p> : null}
    </div>
  )
}

export function ApplyPatchToolActivity(props: ApplyPatchToolActivityProps) {
  return <ApplyPatchToolActivityGroup items={[props]} projectId={props.projectId} />
}

export function ApplyPatchToolActivityGroup({
  items,
  projectId
}: ApplyPatchToolActivityGroupProps) {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  const isPending = items.some((item) => {
    const status = getStatus(item)
    return status === 'waiting' || status === 'running'
  })

  return (
    <AgentActivityDisclosure
      className="agent-activity--apply-patch"
      hasDetails
      icon={Pencil}
      isPending={isPending}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details apply-patch-activity__details">
        {items.map((item) => (
          <ApplyPatchFileRow item={item} key={item.call.id} projectId={projectId} />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
