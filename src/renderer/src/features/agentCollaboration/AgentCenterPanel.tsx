import type { AgentDisplayStatusView, AgentSummary } from '@mycopilot/protocol'
import { Bot, ChevronLeft, Settings2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import { useRightSidebarRuntimeContext } from '../rightSidebar/RightSidebarRuntimeContext'
import type { RightSidebarModulePageState } from '../rightSidebar/rightSidebarTypes'
import './AgentCenterPanel.css'

interface AgentCenterPanelProps {
  onNavigate: (state: Extract<RightSidebarModulePageState, { kind: 'agent-center' }>) => void
  pageState: Extract<RightSidebarModulePageState, { kind: 'agent-center' }> | null
}

const ACTIVE_STATUSES = new Set<AgentDisplayStatusView>(['queued', 'running', 'waiting_approval'])
const INITIAL_ACTIVE_COUNT = 4
const INITIAL_ENDED_COUNT = 10

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
  const detailAgentId = state?.view === 'detail' ? state.agentId : undefined
  const [showAllActive, setShowAllActive] = useState(false)
  const [showAllEnded, setShowAllEnded] = useState(false)
  const [relativeTimeNow, setRelativeTimeNow] = useState(() => Date.now())
  const listElementRef = useRef<HTMLDivElement | null>(null)
  const listScrollTopRef = useRef(0)
  const rowElementsRef = useRef(new Map<string, HTMLButtonElement>())
  const restoreAgentIdRef = useRef<string | null>(null)

  useEffect(() => {
    setShowAllActive(false)
    setShowAllEnded(false)
    listScrollTopRef.current = 0
    restoreAgentIdRef.current = null
    rowElementsRef.current.clear()
  }, [rootConversationId])

  useEffect(() => {
    const interval = window.setInterval(() => setRelativeTimeNow(Date.now()), 60_000)
    return () => window.clearInterval(interval)
  }, [])

  useEffect(() => {
    if (!detailAgentId) return
    const detailAgent = children.find((agent) => agent.agentId === detailAgentId)
    if (!detailAgent) return
    if (ACTIVE_STATUSES.has(detailAgent.displayStatus)) setShowAllActive(true)
    else setShowAllEnded(true)
  }, [children, detailAgentId])

  useEffect(() => {
    if (state?.view !== 'list' || restoreAgentIdRef.current === null) return
    const restoreAgentId = restoreAgentIdRef.current
    const frame = window.requestAnimationFrame(() => {
      if (listElementRef.current) listElementRef.current.scrollTop = listScrollTopRef.current
      rowElementsRef.current.get(restoreAgentId)?.focus()
      restoreAgentIdRef.current = null
    })
    return () => window.cancelAnimationFrame(frame)
  }, [state?.view])

  if (!rootConversationId || !validTree) {
    return <div className="agent-center__state">{t('agentCenter.unavailable')}</div>
  }

  if (state?.view === 'detail' && state.agentId) {
    const agent = children.find((candidate) => candidate.agentId === state.agentId)
    if (agent) {
      const agentLabelsById = Object.fromEntries(
        validTree.agents.map((candidate) => [candidate.agentId, candidate.taskName])
      )
      return (
        <div className="agent-center agent-center--detail">
          <header className="agent-center__detail-header">
            <button
              aria-label={t('agentCenter.back')}
              className="agent-center__back"
              onClick={() => {
                restoreAgentIdRef.current = agent.agentId
                onNavigate({ kind: 'agent-center', rootConversationId, view: 'list' })
              }}
              type="button"
            >
              <ChevronLeft aria-hidden="true" />
            </button>
            <span className="agent-center__avatar agent-center__avatar--detail" aria-hidden="true">
              <Bot />
            </span>
            <div className="agent-center__detail-copy">
              <strong title={agent.taskName}>{agent.taskName}</strong>
              <span title={baseModelLabel(agent, t)}>{baseModelLabel(agent, t)}</span>
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
    <div className="agent-center" ref={listElementRef}>
      <AgentGroup
        agents={active}
        emptyLabel={t('agentCenter.noActive')}
        expanded={showAllActive}
        headerAction={
          onOpenAgentTemplates ? (
            <button
              aria-label={t('agentCenter.manageTemplates')}
              className="agent-center__templates"
              onClick={onOpenAgentTemplates}
              title={t('agentCenter.manageTemplates')}
              type="button"
            >
              <Settings2 aria-hidden="true" />
            </button>
          ) : null
        }
        initialCount={INITIAL_ACTIVE_COUNT}
        language={language}
        onExpand={() => setShowAllActive(true)}
        onOpen={(agentId) => {
          listScrollTopRef.current = listElementRef.current?.scrollTop ?? 0
          onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
        }}
        registerRow={(agentId, element) => {
          if (element) rowElementsRef.current.set(agentId, element)
          else rowElementsRef.current.delete(agentId)
        }}
        relativeTimeNow={relativeTimeNow}
        title={t('agentCenter.active')}
      />

      <AgentGroup
        agents={completed}
        emptyLabel={t('agentCenter.noCompleted')}
        expanded={showAllEnded}
        initialCount={INITIAL_ENDED_COUNT}
        language={language}
        onExpand={() => setShowAllEnded(true)}
        onOpen={(agentId) => {
          listScrollTopRef.current = listElementRef.current?.scrollTop ?? 0
          onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
        }}
        registerRow={(agentId, element) => {
          if (element) rowElementsRef.current.set(agentId, element)
          else rowElementsRef.current.delete(agentId)
        }}
        relativeTimeNow={relativeTimeNow}
        title={t('agentCenter.ended')}
      />
    </div>
  )
}

function AgentGroup({
  agents,
  emptyLabel,
  expanded,
  headerAction,
  initialCount,
  language,
  onExpand,
  onOpen,
  registerRow,
  relativeTimeNow,
  title
}: {
  agents: readonly AgentSummary[]
  emptyLabel: string
  expanded: boolean
  headerAction?: ReactNode
  initialCount: number
  language: string
  onExpand: () => void
  onOpen: (agentId: string) => void
  registerRow: (agentId: string, element: HTMLButtonElement | null) => void
  relativeTimeNow: number
  title: string
}) {
  const { t } = useFrontendConfig()
  const visibleAgents = expanded ? agents : agents.slice(0, initialCount)
  const hiddenCount = agents.length - visibleAgents.length
  return (
    <section className="agent-center__group" aria-label={title}>
      <div className="agent-center__group-heading">
        <h3>
          <span>{title}</span>
          <span aria-hidden="true">·</span>
          <span>{agents.length}</span>
        </h3>
        {headerAction}
      </div>
      <AgentRows
        agents={visibleAgents}
        language={language}
        onOpen={onOpen}
        registerRow={registerRow}
        relativeTimeNow={relativeTimeNow}
      />
      {hiddenCount > 0 ? (
        <button className="agent-center__show-more" onClick={onExpand} type="button">
          {formatTranslation(t, 'agentCenter.showMore', { count: hiddenCount })}
        </button>
      ) : null}
      {agents.length === 0 ? <p className="agent-center__empty">{emptyLabel}</p> : null}
    </section>
  )
}

function AgentRows({
  agents,
  language,
  onOpen,
  registerRow,
  relativeTimeNow
}: {
  agents: readonly AgentSummary[]
  language: string
  onOpen: (agentId: string) => void
  registerRow: (agentId: string, element: HTMLButtonElement | null) => void
  relativeTimeNow: number
}) {
  const { t } = useFrontendConfig()
  return (
    <div className="agent-center__list">
      {agents.map((agent) => {
        const status = statusLabel(agent.displayStatus, t)
        const modelLabel = baseModelLabel(agent, t)
        return (
          <button
            aria-label={formatTranslation(t, 'agentCenter.openAgent', {
              name: agent.taskName,
              model: modelLabel,
              status
            })}
            className="agent-center__row"
            data-agent-id={agent.agentId}
            data-status={agent.displayStatus}
            key={agent.agentId}
            onClick={() => onOpen(agent.agentId)}
            ref={(element) => registerRow(agent.agentId, element)}
            title={`${agent.taskName} · ${modelLabel}`}
            type="button"
          >
            <span className="agent-center__avatar" aria-hidden="true">
              <Bot />
            </span>
            <span className="agent-center__row-copy">
              <strong title={agent.taskName}>{agent.taskName}</strong>
              <span title={modelLabel}>{modelLabel}</span>
            </span>
            <span className="agent-center__row-meta">
              <time dateTime={safeDateTime(agent.latestActivityAt)}>
                {formatRecentActivity(agent.latestActivityAt, relativeTimeNow, language, t)}
              </time>
            </span>
          </button>
        )
      })}
    </div>
  )
}

function statusLabel(status: AgentDisplayStatusView, t: Translate): string {
  switch (status) {
    case 'queued':
      return t('agentCenter.status.queued')
    case 'running':
      return t('agentCenter.status.running')
    case 'waiting_approval':
      return t('agentCenter.status.waitingApproval')
    case 'latest_completed':
      return t('agentCenter.status.completed')
    case 'latest_failed':
      return t('agentCenter.status.failed')
    case 'latest_interrupted':
      return t('agentCenter.status.interrupted')
    case 'latest_outcome_unknown':
      return t('agentCenter.status.outcomeUnknown')
    case 'archived':
      return t('agentCenter.status.archived')
    case 'disabled':
      return t('agentCenter.status.disabled')
    case 'idle':
      return t('agentCenter.status.idle')
  }
}

function baseModelLabel(agent: AgentSummary, t: Translate): string {
  const displayName = agent.model?.displayName.trim()
  if (displayName) return displayName
  const modelConfigId = agent.model?.modelConfigId.trim()
  return modelConfigId || t('agentCenter.modelUnavailable')
}

function formatRecentActivity(
  timestamp: number,
  now: number,
  language: string,
  t: Translate
): string {
  if (!Number.isFinite(timestamp) || timestamp <= 0) return t('agentCenter.timeUnknown')
  const delta = timestamp - now
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
