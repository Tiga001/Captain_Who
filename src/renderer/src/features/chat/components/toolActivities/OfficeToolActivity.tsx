// Renderer UI for native Office tool activity. It deliberately exposes only user-facing action
// summaries and never renders engine, staging, normalized path, or frozen execution metadata.

import { FileCheck2, FilePenLine, FilePlus2, FileSearch, LoaderCircle, XCircle } from 'lucide-react'
import type { AgentToolCall } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import type { ChatAgentRunView } from '../../chatTypes'
import { getOfficeActivityView, type AgentActivityStatus } from '../../skillOfficeActivity'
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
  const hasDetails = Boolean(view.detail)

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
          <p>{view.detail}</p>
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
  const status = aggregateOfficeStatus(views.map((view) => view.status))
  const kind = documentKindLabel(firstView.documentKind, t)
  const details = views.flatMap((view) =>
    view.detail ? [{ id: view.call.id, text: view.detail }] : []
  )
  const label = officeGroupLabel(
    officeLabel(status, firstView.mode, kind, t),
    firstView.mode,
    views.length,
    t
  )

  return (
    <AgentActivityDisclosure
      className="agent-activity--office"
      hasDetails={details.length > 0}
      icon={officeIcon(status, firstView.mode)}
      isPending={status === 'running' || status === 'waiting'}
      label={label}
    >
      {details.length > 0 && (
        <div className="agent-activity__details office-activity__details">
          {details.map((detail) => (
            <p key={detail.id}>{detail.text}</p>
          ))}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
