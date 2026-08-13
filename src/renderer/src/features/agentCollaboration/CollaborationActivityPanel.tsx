import {
  Archive,
  Ban,
  Bot,
  CheckCircle2,
  ChevronRight,
  CircleDot,
  CircleHelp,
  CircleOff,
  Clock3,
  LoaderCircle,
  ShieldAlert,
  TriangleAlert
} from 'lucide-react'
import { useMemo } from 'react'
import type { AgentDisplayStatusView, AgentSummary } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import './CollaborationActivityPanel.css'

export interface CollaborationActivityPanelProps {
  agents: readonly AgentSummary[]
  onOpenAgent: (agentId: string) => void
}

interface AgentStatusPresentation {
  label: string
  tone: 'active' | 'danger' | 'muted' | 'success' | 'warning'
  icon: typeof CircleDot
}

const STATUS_PRIORITY: Record<AgentDisplayStatusView, number> = {
  waiting_approval: 0,
  running: 1,
  queued: 2,
  idle: 3,
  latest_failed: 4,
  latest_outcome_unknown: 5,
  latest_interrupted: 6,
  latest_completed: 7,
  archived: 8,
  disabled: 9
}

function getStatusPresentation(
  status: AgentDisplayStatusView,
  t: Translate
): AgentStatusPresentation {
  switch (status) {
    case 'queued':
      return { icon: Clock3, label: t('collaboration.activity.status.queued'), tone: 'active' }
    case 'running':
      return {
        icon: LoaderCircle,
        label: t('collaboration.activity.status.running'),
        tone: 'active'
      }
    case 'waiting_approval':
      return {
        icon: ShieldAlert,
        label: t('collaboration.activity.status.waitingApproval'),
        tone: 'warning'
      }
    case 'latest_completed':
      return {
        icon: CheckCircle2,
        label: t('collaboration.activity.status.completed'),
        tone: 'success'
      }
    case 'latest_failed':
      return {
        icon: TriangleAlert,
        label: t('collaboration.activity.status.failed'),
        tone: 'danger'
      }
    case 'latest_interrupted':
      return {
        icon: Ban,
        label: t('collaboration.activity.status.interrupted'),
        tone: 'muted'
      }
    case 'latest_outcome_unknown':
      return {
        icon: CircleHelp,
        label: t('collaboration.activity.status.outcomeUnknown'),
        tone: 'warning'
      }
    case 'archived':
      return { icon: Archive, label: t('collaboration.activity.status.archived'), tone: 'muted' }
    case 'disabled':
      return { icon: CircleOff, label: t('collaboration.activity.status.disabled'), tone: 'muted' }
    case 'idle':
      return { icon: CircleDot, label: t('collaboration.activity.status.idle'), tone: 'muted' }
  }
}

function isActive(status: AgentDisplayStatusView): boolean {
  return status === 'queued' || status === 'running' || status === 'waiting_approval'
}

function formatRecentActivity(timestamp: number, language: string, t: Translate): string {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return t('collaboration.activity.time.unknown')
  const delta = timestamp - Date.now()
  const absoluteDelta = Math.abs(delta)
  if (absoluteDelta < 60_000) return t('collaboration.activity.time.now')

  const formatter = new Intl.RelativeTimeFormat(language, { numeric: 'auto' })
  if (absoluteDelta < 3_600_000) return formatter.format(Math.round(delta / 60_000), 'minute')
  if (absoluteDelta < 86_400_000) return formatter.format(Math.round(delta / 3_600_000), 'hour')
  return formatter.format(Math.round(delta / 86_400_000), 'day')
}

function formatDateTime(timestamp: number): string | undefined {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return undefined
  const date = new Date(timestamp)
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString()
}

/**
 * Compact root-chat projection of the authoritative Agent tree.
 *
 * The durable collaboration log only invalidates this view; rows are never manufactured from
 * model prose, generic Tool JSON, or Mailbox payloads. Keeping one row per stable Agent identity
 * makes replay, duplicate notification, and reconnect behavior naturally idempotent.
 */
export function CollaborationActivityPanel({
  agents,
  onOpenAgent
}: CollaborationActivityPanelProps) {
  const { language, t } = useFrontendConfig()
  const children = useMemo(() => {
    const byAgentId = new Map<string, AgentSummary>()
    for (const agent of agents) {
      if (agent.parentAgentId === null) continue
      const current = byAgentId.get(agent.agentId)
      if (!current || agent.latestActivityAt >= current.latestActivityAt) {
        byAgentId.set(agent.agentId, agent)
      }
    }

    return [...byAgentId.values()].sort((left, right) => {
      const statusDifference =
        STATUS_PRIORITY[left.displayStatus] - STATUS_PRIORITY[right.displayStatus]
      if (statusDifference !== 0) return statusDifference
      if (left.latestActivityAt !== right.latestActivityAt) {
        return right.latestActivityAt - left.latestActivityAt
      }
      return left.agentId.localeCompare(right.agentId)
    })
  }, [agents])

  if (children.length === 0) return null

  const activeCount = children.filter((agent) => isActive(agent.displayStatus)).length

  return (
    <section
      aria-label={t('collaboration.activity.title')}
      className="collaboration-activity"
      data-testid="collaboration-activity"
    >
      <header className="collaboration-activity__header">
        <span className="collaboration-activity__title">
          <Bot aria-hidden="true" />
          {t('collaboration.activity.title')}
        </span>
        {activeCount > 0 && (
          <span className="collaboration-activity__active-count">
            {formatTranslation(t, 'collaboration.activity.activeCount', { count: activeCount })}
          </span>
        )}
      </header>

      <div className="collaboration-activity__list">
        {children.map((agent) => {
          const status = getStatusPresentation(agent.displayStatus, t)
          const StatusIcon = status.icon
          return (
            <button
              aria-label={formatTranslation(t, 'collaboration.activity.openAgent', {
                name: agent.taskName
              })}
              className="collaboration-activity__agent"
              data-agent-id={agent.agentId}
              data-status={agent.displayStatus}
              key={agent.agentId}
              onClick={() => onOpenAgent(agent.agentId)}
              type="button"
            >
              <span className="collaboration-activity__status" data-tone={status.tone}>
                <StatusIcon
                  aria-hidden="true"
                  className={agent.displayStatus === 'running' ? 'is-spinning' : undefined}
                />
              </span>
              <span className="collaboration-activity__identity">
                <strong title={agent.taskName}>{agent.taskName}</strong>
                <span title={agent.taskPath}>{agent.taskPath}</span>
              </span>
              <span className="collaboration-activity__meta">
                <span aria-live="polite" data-tone={status.tone}>
                  {status.label}
                </span>
                <span aria-hidden="true">·</span>
                <span>
                  {agent.model?.displayName ?? t('collaboration.activity.modelUnavailable')}
                </span>
                <span aria-hidden="true">·</span>
                <time dateTime={formatDateTime(agent.latestActivityAt)}>
                  {formatRecentActivity(agent.latestActivityAt, language, t)}
                </time>
              </span>
              <ChevronRight aria-hidden="true" className="collaboration-activity__chevron" />
            </button>
          )
        })}
      </div>
    </section>
  )
}
