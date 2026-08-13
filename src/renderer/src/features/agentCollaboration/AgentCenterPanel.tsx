import type { AgentDisplayStatusView, AgentSummary } from '@mycopilot/protocol'
import {
  Archive,
  Ban,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  CircleHelp,
  CircleOff,
  Clock3,
  LoaderCircle,
  Settings2,
  ShieldAlert,
  TriangleAlert
} from 'lucide-react'
import { useMemo } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import type { Translate } from '../../config/translationFormat'
import { useRightSidebarRuntimeContext } from '../rightSidebar/RightSidebarRuntimeContext'
import type { RightSidebarModulePageState } from '../rightSidebar/rightSidebarTypes'
import './AgentCenterPanel.css'

interface AgentCenterPanelProps {
  onNavigate: (state: Extract<RightSidebarModulePageState, { kind: 'agent-center' }>) => void
  pageState: Extract<RightSidebarModulePageState, { kind: 'agent-center' }> | null
}

const ACTIVE_STATUSES = new Set<AgentDisplayStatusView>(['queued', 'running', 'waiting_approval'])

export function AgentCenterPanel({ onNavigate, pageState }: AgentCenterPanelProps) {
  const { language, t } = useFrontendConfig()
  const { activeConversationId, collaborationSnapshot, onOpenAgentTemplates, renderAgentObserver } =
    useRightSidebarRuntimeContext()
  const tree = collaborationSnapshot?.tree
  const rootConversationId = activeConversationId
  const validTree =
    tree && rootConversationId && tree.rootConversationId === rootConversationId ? tree : null
  const children = useMemo(
    () =>
      validTree
        ? validTree.agents
            .filter((agent) => agent.parentAgentId !== null)
            .sort(
              (left, right) =>
                right.latestActivityAt - left.latestActivityAt ||
                left.agentId.localeCompare(right.agentId)
            )
        : [],
    [validTree]
  )
  const state =
    pageState?.rootConversationId === rootConversationId
      ? pageState
      : rootConversationId
        ? ({ kind: 'agent-center', rootConversationId, view: 'list' } as const)
        : null

  if (!rootConversationId || !validTree) {
    return <div className="agent-center__state">{t('agentCenter.unavailable')}</div>
  }

  if (state?.view === 'detail' && state.agentId) {
    const agent = children.find((candidate) => candidate.agentId === state.agentId)
    if (agent) {
      const parent = validTree.agents.find((candidate) => candidate.agentId === agent.parentAgentId)
      const agentLabelsById = Object.fromEntries(
        validTree.agents.map((candidate) => [candidate.agentId, candidate.taskName])
      )
      return (
        <div className="agent-center agent-center--detail">
          <header className="agent-center__detail-header">
            <button
              aria-label={t('agentCenter.back')}
              className="agent-center__back"
              onClick={() => onNavigate({ kind: 'agent-center', rootConversationId, view: 'list' })}
              type="button"
            >
              <ChevronLeft aria-hidden="true" />
            </button>
            <div className="agent-center__detail-copy">
              <strong title={agent.taskName}>{agent.taskName}</strong>
              <span>
                {statusPresentation(agent.displayStatus, t).label}
                {parent ? ` · ${t('agentCenter.from')} ${parent.taskName}` : ''}
              </span>
            </div>
          </header>
          <div className="agent-center__observer" data-agent-id={agent.agentId}>
            {renderAgentObserver ? (
              renderAgentObserver({
                agent,
                agentLabelsById,
                invalidationVersion: `${collaborationSnapshot?.hydrationRevision ?? 0}:${
                  collaborationSnapshot?.agentInvalidationSequences[agent.agentId] ?? 0
                }`,
                rootConversationId
              })
            ) : (
              <div className="agent-center__state">{t('agentCenter.observerUnavailable')}</div>
            )}
          </div>
        </div>
      )
    }
  }

  const active = children.filter((agent) => ACTIVE_STATUSES.has(agent.displayStatus))
  const completed = children.filter((agent) => !ACTIVE_STATUSES.has(agent.displayStatus))

  return (
    <div className="agent-center">
      <header className="agent-center__header">
        <div>
          <h2>{t('agentCenter.title')}</h2>
          <p>{t('agentCenter.description')}</p>
        </div>
        {onOpenAgentTemplates ? (
          <button
            aria-label={t('agentCenter.manageTemplates')}
            className="agent-center__templates"
            onClick={onOpenAgentTemplates}
            title={t('agentCenter.manageTemplates')}
            type="button"
          >
            <Settings2 aria-hidden="true" />
          </button>
        ) : null}
      </header>

      <AgentGroup
        agents={active}
        emptyLabel={t('agentCenter.noActive')}
        language={language}
        onOpen={(agentId) =>
          onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
        }
        title={t('agentCenter.active')}
      />

      <details className="agent-center__completed" open>
        <summary>
          <span>{t('agentCenter.completed')}</span>
          <span>{completed.length}</span>
        </summary>
        <AgentRows
          agents={completed}
          language={language}
          onOpen={(agentId) =>
            onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
          }
        />
        {completed.length === 0 ? (
          <p className="agent-center__empty">{t('agentCenter.noCompleted')}</p>
        ) : null}
      </details>
    </div>
  )
}

function AgentGroup({
  agents,
  emptyLabel,
  language,
  onOpen,
  title
}: {
  agents: readonly AgentSummary[]
  emptyLabel: string
  language: string
  onOpen: (agentId: string) => void
  title: string
}) {
  return (
    <section className="agent-center__group" aria-label={title}>
      <h3>
        <span>{title}</span>
        <span>{agents.length}</span>
      </h3>
      <AgentRows agents={agents} language={language} onOpen={onOpen} />
      {agents.length === 0 ? <p className="agent-center__empty">{emptyLabel}</p> : null}
    </section>
  )
}

function AgentRows({
  agents,
  language,
  onOpen
}: {
  agents: readonly AgentSummary[]
  language: string
  onOpen: (agentId: string) => void
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="agent-center__list">
      {agents.map((agent) => {
        const status = statusPresentation(agent.displayStatus, t)
        const StatusIcon = status.icon
        return (
          <button
            aria-label={`${t('agentCenter.open')} ${agent.taskName}`}
            className="agent-center__row"
            data-status={agent.displayStatus}
            key={agent.agentId}
            onClick={() => onOpen(agent.agentId)}
            type="button"
          >
            <span className="agent-center__status" data-tone={status.tone}>
              <StatusIcon
                aria-hidden="true"
                className={agent.displayStatus === 'running' ? 'is-spinning' : undefined}
              />
            </span>
            <span className="agent-center__row-copy">
              <strong title={agent.taskName}>{agent.taskName}</strong>
              <span title={agent.taskPath}>{agent.taskPath}</span>
              <small>
                <span data-tone={status.tone}>{status.label}</span>
                <span aria-hidden="true"> · </span>
                <span>{agent.model?.displayName ?? t('agentCenter.modelUnavailable')}</span>
                <span aria-hidden="true"> · </span>
                <time dateTime={safeDateTime(agent.latestActivityAt)}>
                  {formatRecentActivity(agent.latestActivityAt, language, t)}
                </time>
              </small>
            </span>
            <ChevronRight aria-hidden="true" />
          </button>
        )
      })}
    </div>
  )
}

function statusPresentation(status: AgentDisplayStatusView, t: Translate) {
  switch (status) {
    case 'queued':
      return { icon: Clock3, label: t('agentCenter.status.queued'), tone: 'active' } as const
    case 'running':
      return { icon: LoaderCircle, label: t('agentCenter.status.running'), tone: 'active' } as const
    case 'waiting_approval':
      return {
        icon: ShieldAlert,
        label: t('agentCenter.status.waitingApproval'),
        tone: 'warning'
      } as const
    case 'latest_completed':
      return {
        icon: CheckCircle2,
        label: t('agentCenter.status.completed'),
        tone: 'success'
      } as const
    case 'latest_failed':
      return { icon: TriangleAlert, label: t('agentCenter.status.failed'), tone: 'danger' } as const
    case 'latest_interrupted':
      return { icon: Ban, label: t('agentCenter.status.interrupted'), tone: 'muted' } as const
    case 'latest_outcome_unknown':
      return {
        icon: CircleHelp,
        label: t('agentCenter.status.outcomeUnknown'),
        tone: 'warning'
      } as const
    case 'archived':
      return { icon: Archive, label: t('agentCenter.status.archived'), tone: 'muted' } as const
    case 'disabled':
      return { icon: CircleOff, label: t('agentCenter.status.disabled'), tone: 'muted' } as const
    case 'idle':
      return { icon: CircleDot, label: t('agentCenter.status.idle'), tone: 'muted' } as const
  }
}

function formatRecentActivity(timestamp: number, language: string, t: Translate): string {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return t('agentCenter.timeUnknown')
  const delta = timestamp - Date.now()
  const absolute = Math.abs(delta)
  if (absolute < 60_000) return t('agentCenter.timeNow')
  const formatter = new Intl.RelativeTimeFormat(language, { numeric: 'auto' })
  if (absolute < 3_600_000) return formatter.format(Math.round(delta / 60_000), 'minute')
  if (absolute < 86_400_000) return formatter.format(Math.round(delta / 3_600_000), 'hour')
  return formatter.format(Math.round(delta / 86_400_000), 'day')
}

function safeDateTime(timestamp: number): string | undefined {
  const value = new Date(timestamp)
  return Number.isFinite(timestamp) && !Number.isNaN(value.getTime())
    ? value.toISOString()
    : undefined
}
