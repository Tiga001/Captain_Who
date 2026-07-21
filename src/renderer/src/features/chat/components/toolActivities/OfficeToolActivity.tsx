// Renderer UI for native Office tool activity. It deliberately exposes only user-facing action
// summaries and never renders engine, staging, normalized path, or frozen execution metadata.

import {
  AlertTriangle,
  CheckCircle2,
  CircleSlash2,
  FileCheck2,
  FilePenLine,
  FilePlus2,
  FileSearch,
  ListChecks,
  LoaderCircle,
  XCircle
} from 'lucide-react'
import type { AgentToolCall } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { formatTranslation } from '../../../../config/translationFormat'
import type { ChatAgentRunView } from '../../chatTypes'
import {
  getOfficeActivityView,
  type AgentActivityStatus,
  type OfficeActivityView
} from '../../skillOfficeActivity'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

export interface OfficeToolActivityGroupItem {
  call: AgentToolCall
  settledStatus?: SettledToolStatus
}

function documentKindLabel(
  kind: 'document' | 'spreadsheet' | 'presentation',
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  if (kind === 'spreadsheet') return t('agent.office.kind.spreadsheet')
  if (kind === 'presentation') return t('agent.office.kind.presentation')
  return t('agent.office.kind.document')
}

function officeLabel(
  status: AgentActivityStatus,
  mode: 'create' | 'edit' | 'view' | 'query' | 'validate' | 'export',
  kind: string,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  if (status === 'cancelled') return formatTranslation(t, 'agent.office.cancelled', { kind })
  if (status === 'rejected') return formatTranslation(t, 'agent.office.rejected', { kind })
  if (status === 'conflict') return formatTranslation(t, 'agent.office.conflict', { kind })
  if (status === 'failed') return formatTranslation(t, 'agent.office.failed', { kind })
  if (status === 'waiting') {
    return formatTranslation(
      t,
      mode === 'create' ? 'agent.office.waitingCreate' : 'agent.office.waitingEdit',
      { kind }
    )
  }
  if (status === 'running') {
    const key =
      mode === 'create'
        ? 'agent.office.runningCreate'
        : mode === 'edit'
          ? 'agent.office.runningEdit'
          : mode === 'export'
            ? 'agent.office.runningExport'
            : 'agent.office.runningView'
    return formatTranslation(t, key, { kind })
  }

  const key =
    mode === 'create'
      ? 'agent.office.created'
      : mode === 'edit'
        ? 'agent.office.edited'
        : mode === 'validate'
          ? 'agent.office.validated'
          : mode === 'query'
            ? 'agent.office.queried'
            : mode === 'export'
              ? 'agent.office.exported'
              : 'agent.office.viewed'
  return formatTranslation(t, key, { kind })
}

function officeIcon(status: AgentActivityStatus, mode: string) {
  if (status === 'running' || status === 'waiting') return LoaderCircle
  if (status === 'failed' || status === 'rejected' || status === 'conflict') return XCircle
  if (mode === 'create') return FilePlus2
  if (mode === 'edit') return FilePenLine
  if (mode === 'validate') return FileCheck2
  return FileSearch
}

function aggregateOfficeStatus(statuses: AgentActivityStatus[]): AgentActivityStatus {
  if (statuses.some((status) => status === 'waiting')) return 'waiting'
  if (statuses.some((status) => status === 'running')) return 'running'
  if (statuses.some((status) => status === 'conflict')) return 'conflict'
  if (statuses.some((status) => status === 'rejected')) return 'rejected'
  if (statuses.some((status) => status === 'failed')) return 'failed'
  if (statuses.some((status) => status === 'cancelled')) return 'cancelled'
  return 'completed'
}

function officeGroupLabel(
  label: string,
  mode: 'create' | 'edit' | 'view' | 'query' | 'validate' | 'export',
  count: number,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  if (count <= 1) return label
  return formatTranslation(
    t,
    mode === 'view' || mode === 'query' || mode === 'validate'
      ? 'agent.office.groupChecks'
      : 'agent.office.groupOperations',
    { count: String(count), label }
  )
}

type OfficeStatusCounts = Record<AgentActivityStatus, number>

const OFFICE_STATUS_ORDER: readonly AgentActivityStatus[] = [
  'waiting',
  'running',
  'completed',
  'failed',
  'conflict',
  'rejected',
  'cancelled'
]

const OFFICE_STATUS_COUNT_KEY: Readonly<Record<AgentActivityStatus, TranslationKey>> = {
  waiting: 'agent.office.groupStatus.waiting',
  running: 'agent.office.groupStatus.running',
  completed: 'agent.office.groupStatus.completed',
  failed: 'agent.office.groupStatus.failed',
  conflict: 'agent.office.groupStatus.conflict',
  rejected: 'agent.office.groupStatus.rejected',
  cancelled: 'agent.office.groupStatus.cancelled'
}

const OFFICE_ITEM_STATUS_KEY: Readonly<Record<AgentActivityStatus, TranslationKey>> = {
  waiting: 'agent.office.itemStatus.waiting',
  running: 'agent.office.itemStatus.running',
  completed: 'agent.office.itemStatus.completed',
  failed: 'agent.office.itemStatus.failed',
  conflict: 'agent.office.itemStatus.conflict',
  rejected: 'agent.office.itemStatus.rejected',
  cancelled: 'agent.office.itemStatus.cancelled'
}

function countOfficeStatuses(statuses: AgentActivityStatus[]): OfficeStatusCounts {
  const counts: OfficeStatusCounts = {
    waiting: 0,
    running: 0,
    completed: 0,
    failed: 0,
    conflict: 0,
    rejected: 0,
    cancelled: 0
  }
  statuses.forEach((status) => {
    counts[status] += 1
  })
  return counts
}

function officeStatusCountLabel(
  status: AgentActivityStatus,
  count: number,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  return formatTranslation(t, OFFICE_STATUS_COUNT_KEY[status], { count: String(count) })
}

function mixedOfficeGroupLabel(
  mode: 'create' | 'edit' | 'view' | 'query' | 'validate' | 'export',
  kind: string,
  counts: OfficeStatusCounts,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const category = formatTranslation(
    t,
    mode === 'view' || mode === 'query' || mode === 'validate'
      ? 'agent.office.groupCheckSummary'
      : 'agent.office.groupOperationSummary',
    { kind }
  )
  const statusParts = OFFICE_STATUS_ORDER.flatMap((status) =>
    counts[status] > 0 ? [officeStatusCountLabel(status, counts[status], t)] : []
  )
  return [category, ...statusParts].join(' · ')
}

function officeItemStatusLabel(
  status: AgentActivityStatus,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  return t(OFFICE_ITEM_STATUS_KEY[status])
}

function OfficeItemStatusIcon({ status }: { status: AgentActivityStatus }) {
  if (status === 'completed') return <CheckCircle2 aria-hidden="true" />
  if (status === 'waiting' || status === 'running') return <LoaderCircle aria-hidden="true" />
  if (status === 'cancelled' || status === 'rejected') return <CircleSlash2 aria-hidden="true" />
  if (status === 'conflict') return <AlertTriangle aria-hidden="true" />
  return <XCircle aria-hidden="true" />
}

function OfficeActivityDetail({
  kind,
  t,
  view
}: {
  kind: string
  t: ReturnType<typeof useFrontendConfig>['t']
  view: OfficeActivityView
}) {
  const summary = view.reason ?? view.error ?? officeLabel(view.status, view.mode, kind, t)
  const showError =
    view.status === 'failed' ||
    view.status === 'conflict' ||
    view.status === 'rejected' ||
    view.status === 'cancelled'
  const secondaryError = showError && view.error !== summary ? view.error : undefined

  return (
    <div className="office-activity__item" data-status={view.status}>
      <span className="office-activity__item-status">
        <OfficeItemStatusIcon status={view.status} />
        <span>{officeItemStatusLabel(view.status, t)}</span>
      </span>
      <div className="office-activity__item-content">
        <p>{summary}</p>
        {secondaryError && <p className="office-activity__item-error">{secondaryError}</p>}
      </div>
    </div>
  )
}

export function OfficeToolActivity({
  call,
  run,
  settledStatus
}: {
  call: AgentToolCall
  run: ChatAgentRunView
  settledStatus?: SettledToolStatus
}) {
  const { t } = useFrontendConfig()
  const view = getOfficeActivityView(run, call, settledStatus)
  if (!view) return null
  const kind = documentKindLabel(view.documentKind, t)
  const hasDetails = Boolean(view.reason || view.error)

  return (
    <AgentActivityDisclosure
      className="agent-activity--office"
      hasDetails={hasDetails}
      icon={officeIcon(view.status, view.mode)}
      isPending={view.status === 'running' || view.status === 'waiting'}
      label={officeLabel(view.status, view.mode, kind, t)}
    >
      {hasDetails && (
        <div className="agent-activity__details office-activity__details">
          <OfficeActivityDetail kind={kind} t={t} view={view} />
        </div>
      )}
    </AgentActivityDisclosure>
  )
}

export function OfficeToolActivityGroup({
  items,
  run
}: {
  items: OfficeToolActivityGroupItem[]
  run: ChatAgentRunView
}) {
  const { t } = useFrontendConfig()
  if (items.length === 0) return null
  if (items.length === 1) {
    return (
      <OfficeToolActivity call={items[0].call} run={run} settledStatus={items[0].settledStatus} />
    )
  }

  const views = items
    .map((item) => getOfficeActivityView(run, item.call, item.settledStatus))
    .filter((view) => view !== undefined)
  if (views.length === 0) return null
  const firstView = views[0]
  const statuses = views.map((view) => view.status)
  const status = aggregateOfficeStatus(statuses)
  const counts = countOfficeStatuses(statuses)
  const mixedStatuses = OFFICE_STATUS_ORDER.filter((candidate) => counts[candidate] > 0).length > 1
  const kind = documentKindLabel(firstView.documentKind, t)
  const label = mixedStatuses
    ? mixedOfficeGroupLabel(firstView.mode, kind, counts, t)
    : officeGroupLabel(
        officeLabel(status, firstView.mode, kind, t),
        firstView.mode,
        views.length,
        t
      )
  const isPending = counts.running > 0 || counts.waiting > 0
  const GroupIcon = mixedStatuses
    ? isPending
      ? LoaderCircle
      : ListChecks
    : officeIcon(status, firstView.mode)

  return (
    <AgentActivityDisclosure
      className="agent-activity--office"
      hasDetails
      icon={GroupIcon}
      isPending={isPending}
      label={label}
    >
      <div className="agent-activity__details office-activity__details">
        {views.map((view) => (
          <OfficeActivityDetail kind={kind} key={view.call.id} t={t} view={view} />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
