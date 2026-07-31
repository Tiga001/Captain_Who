import {
  Ban,
  CheckCircle2,
  CircleHelp,
  Clock3,
  LoaderCircle,
  PlugZap,
  XCircle,
  type LucideIcon
} from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import { toSafeMcpDisplayText } from '../../../mcp/mcpSafeDisplay'
import type { ChatMcpToolInvocationView } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import './McpToolActivity.css'

type McpActivityStatus =
  'waiting' | 'running' | 'succeeded' | 'failed' | 'rejected' | 'cancelled' | 'outcome_unknown'

interface McpStatusPresentation {
  Icon: LucideIcon
  pending: boolean
}

export interface McpToolActivityGroupProps {
  items: ChatMcpToolInvocationView[]
}

function safePlainText(value: string, maximumLength: number) {
  const normalized = toSafeMcpDisplayText(value, maximumLength).trim()
  if (!normalized) return '\u2014'
  return normalized
}

function safeOptionalText(value: string | undefined, maximumLength: number) {
  if (!value) return undefined
  const normalized = toSafeMcpDisplayText(value, maximumLength).trim()
  return normalized || undefined
}

function getMcpActivityStatus(invocation: ChatMcpToolInvocationView): McpActivityStatus {
  switch (invocation.state) {
    case 'pending_approval':
      return 'waiting'
    case 'approved':
    case 'dispatching':
    case 'running':
      return 'running'
    case 'completed':
      return invocation.isError || invocation.outcome === 'tool_error' ? 'failed' : 'succeeded'
    case 'rejected':
    case 'policy_denied':
      return 'rejected'
    case 'cancelled':
      return 'cancelled'
    case 'outcome_unknown':
      return 'outcome_unknown'
    case 'expired':
    case 'payload_unavailable':
    case 'failed':
      return 'failed'
  }
}

function getStatusPresentation(invocation: ChatMcpToolInvocationView): McpStatusPresentation {
  const status = getMcpActivityStatus(invocation)

  switch (status) {
    case 'waiting':
      return { Icon: Clock3, pending: true }
    case 'running':
      return { Icon: LoaderCircle, pending: true }
    case 'succeeded':
      return { Icon: CheckCircle2, pending: false }
    case 'failed':
      return { Icon: XCircle, pending: false }
    case 'rejected':
    case 'cancelled':
      return { Icon: Ban, pending: false }
    case 'outcome_unknown':
      return { Icon: CircleHelp, pending: false }
  }
}

function getSingleStatusKey(invocation: ChatMcpToolInvocationView) {
  switch (invocation.state) {
    case 'pending_approval':
      return 'agent.mcp.activity.single.waiting' as const
    case 'approved':
    case 'dispatching':
    case 'running':
      return 'agent.mcp.activity.single.running' as const
    case 'completed':
      return invocation.isError || invocation.outcome === 'tool_error'
        ? ('agent.mcp.activity.single.failed' as const)
        : ('agent.mcp.activity.single.completed' as const)
    case 'cancelled':
      return 'agent.mcp.activity.single.cancelled' as const
    case 'rejected':
      return 'agent.mcp.activity.single.rejected' as const
    case 'expired':
      return 'agent.mcp.activity.single.expired' as const
    case 'payload_unavailable':
      return 'agent.mcp.activity.single.payloadUnavailable' as const
    case 'policy_denied':
      return 'agent.mcp.activity.single.policyDenied' as const
    case 'outcome_unknown':
      return 'agent.mcp.activity.single.outcomeUnknown' as const
    case 'failed':
      return 'agent.mcp.activity.single.failed' as const
  }
}

function getSingleLabel(invocation: ChatMcpToolInvocationView, t: Translate, compact: boolean) {
  const tool = safePlainText(invocation.rawToolName, 1024)
  const server = safePlainText(invocation.serverDisplayName, 128)
  const key = getSingleStatusKey(invocation)
  const summary = compact
    ? formatTranslation(t, `${key}.compact`, { tool })
    : formatTranslation(t, key, { server, tool })

  return invocation.outputTruncated
    ? [summary, t('agent.mcp.activity.summary.outputTruncated')].join(t('agent.separator'))
    : summary
}

function getReason(invocation: ChatMcpToolInvocationView, t: Translate) {
  const rejectionReason =
    invocation.state === 'rejected' ? safeOptionalText(invocation.rejectionReason, 512) : undefined
  const displayReason = safeOptionalText(invocation.displayReason, 512)
  return rejectionReason ?? displayReason ?? t('agent.mcp.activity.reasonUnavailable')
}

function getDuration(invocation: ChatMcpToolInvocationView, t: Translate) {
  if (invocation.durationMs !== undefined) {
    return formatTranslation(t, 'agent.mcp.activity.durationMs', {
      duration: Math.max(0, Math.trunc(invocation.durationMs)).toLocaleString()
    })
  }

  if (invocation.state === 'pending_approval' || invocation.state === 'approved') {
    return t('agent.mcp.activity.durationNotStarted')
  }
  if (invocation.state === 'dispatching' || invocation.state === 'running') {
    return t('agent.mcp.activity.durationInProgress')
  }
  if (
    invocation.state === 'rejected' ||
    invocation.state === 'expired' ||
    invocation.state === 'payload_unavailable' ||
    invocation.state === 'policy_denied' ||
    (invocation.state === 'cancelled' &&
      invocation.dispatchCertainty === 'definitely_not_dispatched')
  ) {
    return t('agent.mcp.activity.durationNotExecuted')
  }
  return t('agent.mcp.activity.durationUnavailable')
}

function getGroupCounts(items: ChatMcpToolInvocationView[]) {
  return items.reduce<Record<McpActivityStatus, number>>(
    (counts, invocation) => {
      counts[getMcpActivityStatus(invocation)] += 1
      return counts
    },
    {
      waiting: 0,
      running: 0,
      succeeded: 0,
      failed: 0,
      rejected: 0,
      cancelled: 0,
      outcome_unknown: 0
    }
  )
}

function getGroupLabel(items: ChatMcpToolInvocationView[], t: Translate) {
  const server = safePlainText(items[0]?.serverDisplayName ?? '', 128)
  const counts = getGroupCounts(items)
  const anyPossiblyDispatched = items.some(
    (invocation) => invocation.dispatchCertainty !== 'definitely_not_dispatched'
  )
  const statusKey =
    counts.running > 0
      ? 'agent.mcp.activity.group.running'
      : counts.waiting > 0
        ? 'agent.mcp.activity.group.waiting'
        : anyPossiblyDispatched
          ? 'agent.mcp.activity.group.completed'
          : 'agent.mcp.activity.group.processed'
  const summary = formatTranslation(t, statusKey, {
    server,
    count: items.length
  })
  const countParts = [
    counts.waiting
      ? formatTranslation(t, 'agent.mcp.activity.group.waitingCount', {
          count: counts.waiting
        })
      : '',
    counts.running
      ? formatTranslation(t, 'agent.mcp.activity.group.runningCount', {
          count: counts.running
        })
      : '',
    counts.succeeded
      ? formatTranslation(t, 'agent.mcp.activity.group.succeededCount', {
          count: counts.succeeded
        })
      : '',
    counts.failed
      ? formatTranslation(t, 'agent.mcp.activity.group.failedCount', {
          count: counts.failed
        })
      : '',
    counts.rejected
      ? formatTranslation(t, 'agent.mcp.activity.group.rejectedCount', {
          count: counts.rejected
        })
      : '',
    counts.cancelled
      ? formatTranslation(t, 'agent.mcp.activity.group.cancelledCount', {
          count: counts.cancelled
        })
      : '',
    counts.outcome_unknown
      ? formatTranslation(t, 'agent.mcp.activity.group.outcomeUnknownCount', {
          count: counts.outcome_unknown
        })
      : '',
    items.some((invocation) => invocation.outputTruncated)
      ? formatTranslation(t, 'agent.mcp.activity.group.outputTruncatedCount', {
          count: items.filter((invocation) => invocation.outputTruncated).length
        })
      : ''
  ].filter(Boolean)

  return [summary, ...countParts].join(t('agent.separator'))
}

function getGroupPresentation(items: ChatMcpToolInvocationView[]): McpStatusPresentation {
  const counts = getGroupCounts(items)
  if (counts.running > 0) return { Icon: LoaderCircle, pending: true }
  if (counts.waiting > 0) return { Icon: Clock3, pending: true }
  if (counts.outcome_unknown > 0) {
    return { Icon: CircleHelp, pending: false }
  }
  if (counts.failed > 0) return { Icon: XCircle, pending: false }
  if (counts.rejected > 0) return { Icon: Ban, pending: false }
  if (counts.cancelled > 0) return { Icon: Ban, pending: false }
  return { Icon: CheckCircle2, pending: false }
}

export function McpToolActivity({
  compact = false,
  invocation
}: {
  compact?: boolean
  invocation?: ChatMcpToolInvocationView
}) {
  const { t } = useFrontendConfig()

  if (!invocation) {
    return (
      <div className="agent-activity mcp-tool-activity" role="status">
        <div className="agent-activity__static-summary">
          <span className="agent-activity__icon">
            <PlugZap aria-hidden="true" />
          </span>
          <span className="agent-activity__label">
            {t('agent.mcp.activity.externalTool')} \u00B7{' '}
            {t('agent.mcp.activity.detailUnavailable')}
          </span>
        </div>
      </div>
    )
  }

  const presentation = getStatusPresentation(invocation)
  const label = getSingleLabel(invocation, t, compact)

  return (
    <>
      <span className="mcp-tool-activity__sr-status" role="status">
        {label}
      </span>
      <AgentActivityDisclosure
        className="mcp-tool-activity"
        hasDetails
        icon={presentation.Icon}
        isPending={presentation.pending}
        label={label}
      >
        <div className="agent-activity__details mcp-tool-activity__details">
          <dl className="mcp-tool-activity__metadata">
            <div>
              <dt>{t('agent.mcp.activity.reason')}</dt>
              <dd>{getReason(invocation, t)}</dd>
            </div>
            <div>
              <dt>{t('agent.mcp.activity.duration')}</dt>
              <dd>{getDuration(invocation, t)}</dd>
            </div>
          </dl>
        </div>
      </AgentActivityDisclosure>
    </>
  )
}

export function McpToolActivityGroup({
  items
}: McpToolActivityGroupProps): React.JSX.Element | null {
  const { t } = useFrontendConfig()
  const firstItem = items[0]
  if (!firstItem) return null
  if (items.length === 1) return <McpToolActivity invocation={firstItem} />

  const presentation = getGroupPresentation(items)

  return (
    <AgentActivityDisclosure
      className="mcp-tool-activity mcp-tool-activity--group"
      hasDetails
      icon={presentation.Icon}
      isPending={presentation.pending}
      label={getGroupLabel(items, t)}
    >
      <div className="agent-activity__details mcp-tool-activity__group-items">
        {items.map((invocation) => (
          <McpToolActivity compact invocation={invocation} key={invocation.invocationId} />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
