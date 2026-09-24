import type { AgentSummary } from '@mycopilot/protocol'
import { ChevronLeft, List, ListTree, Network, Settings2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { Tooltip } from '../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import { useRightSidebarRuntimeContext } from '../rightSidebar/RightSidebarRuntimeContext'
import type { RightSidebarModulePageState } from '../rightSidebar/rightSidebarTypes'
import { AgentAvatar } from './AgentAvatar'
import { AgentTreeView, type AgentTreeLayout } from './AgentTreeView'
import { ACTIVE_STATUSES, baseModelLabel, statusLabel } from './agentCenterLabels'
import './AgentCenterPanel.css'

interface AgentCenterPanelProps {
  onNavigate: (state: Extract<RightSidebarModulePageState, { kind: 'agent-center' }>) => void
  pageState: Extract<RightSidebarModulePageState, { kind: 'agent-center' }> | null
}

const INITIAL_ACTIVE_COUNT = 4
const INITIAL_ENDED_COUNT = 10

const VIEW_OPTIONS = [
  { id: 'list', label: 'agentCenter.switchToList', icon: List },
  { id: 'diagram', label: 'agentCenter.switchToDiagram', icon: Network },
  { id: 'outline', label: 'agentCenter.switchToOutline', icon: ListTree }
] as const

export function AgentCenterPanel({ onNavigate, pageState }: AgentCenterPanelProps) {
  const { language, t } = useFrontendConfig()
  const {
    activeConversationId,
    collaborationSnapshot,
    onOpenAgentTemplates,
    onOpenProfile,
    onOpenAgentRootConversation,
    renderAgentObserver
  } = useRightSidebarRuntimeContext()
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
  const [homeView, setHomeView] = useState<'list' | 'tree'>('list')
  const [treeLayout, setTreeLayout] = useState<AgentTreeLayout>('diagram')
  const [userCollapsed, setUserCollapsed] = useState(false)
  const [collapsedAgentIds, setCollapsedAgentIds] = useState<Set<string>>(() => new Set())
  const treeElementRef = useRef<HTMLDivElement | null>(null)
  const treeScrollRef = useRef({ diagram: { top: 0, left: 0 }, outline: { top: 0, left: 0 } })
  const [showAllActive, setShowAllActive] = useState(false)
  const [showAllEnded, setShowAllEnded] = useState(false)
  const [relativeTimeNow, setRelativeTimeNow] = useState(() => Date.now())
  const listElementRef = useRef<HTMLDivElement | null>(null)
  const listScrollTopRef = useRef(0)
  const rowElementsRef = useRef(new Map<string, HTMLButtonElement>())
  const restoreAgentIdRef = useRef<string | null>(null)

  useEffect(() => {
    setHomeView('list')
    setTreeLayout('diagram')
    setUserCollapsed(false)
    setCollapsedAgentIds(new Set())
    treeScrollRef.current = { diagram: { top: 0, left: 0 }, outline: { top: 0, left: 0 } }
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
    if (state?.view !== 'list') return
    const restoreAgentId = restoreAgentIdRef.current
    const frame = window.requestAnimationFrame(() => {
      if (homeView === 'tree' && treeElementRef.current) {
        treeElementRef.current.scrollTop = treeScrollRef.current[treeLayout].top
        treeElementRef.current.scrollLeft = treeScrollRef.current[treeLayout].left
      } else if (listElementRef.current) {
        listElementRef.current.scrollTop = listScrollTopRef.current
      }
      if (restoreAgentId)
        rowElementsRef.current.get(restoreAgentId)?.focus({ preventScroll: homeView === 'tree' })
      restoreAgentIdRef.current = null
    })
    return () => window.cancelAnimationFrame(frame)
  }, [state?.view, homeView, treeLayout])

  if (!rootConversationId || !validTree) {
    return <div className="agent-center__state">{t('agentCenter.unavailable')}</div>
  }

  if (state?.view === 'detail' && state.agentId) {
    const agent = children.find((candidate) => candidate.agentId === state.agentId)
    if (agent) {
      const agentLabelsById = Object.fromEntries(
        validTree.agents.map((candidate) => [candidate.agentId, candidate.taskName])
      )
      const treeAgentIds = new Set(validTree.agents.map((candidate) => candidate.agentId))
      const activities = (collaborationSnapshot?.activities ?? []).filter(
        (activity) =>
          activity.ownerAgentId === agent.agentId &&
          activity.ownerConversationId === agent.conversationId &&
          treeAgentIds.has(activity.agentId)
      )
      return (
        <div className="agent-center agent-center--detail">
          <header className="agent-center__detail-header">
            <Tooltip content={t('agentCenter.back')} preferredPlacement="bottom">
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
            </Tooltip>
            <AgentAvatar
              agentId={agent.agentId}
              className="agent-center__avatar agent-center__avatar--detail"
            />
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
                activities,
                collaborationTreeAgentIds: [...treeAgentIds],
                invalidationVersion: `${collaborationSnapshot?.hydrationRevision ?? 0}:${
                  collaborationSnapshot?.agentInvalidationSequences[agent.agentId] ?? 0
                }`,
                onOpenAgent: (agentId) => {
                  if (agentId === validTree.rootAgentId) {
                    onOpenAgentRootConversation?.(rootConversationId)
                    return
                  }
                  onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
                },
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

  const rememberScroll = () => {
    if (homeView === 'tree') {
      treeScrollRef.current[treeLayout] = {
        top: treeElementRef.current?.scrollTop ?? 0,
        left: treeElementRef.current?.scrollLeft ?? 0
      }
    } else {
      listScrollTopRef.current = listElementRef.current?.scrollTop ?? 0
    }
  }
  const selectedView = homeView === 'list' ? 'list' : treeLayout
  const headerActions = (
    <div className="agent-center__view-actions">
      {VIEW_OPTIONS.map(({ id, label, icon: Icon }) => (
        <Tooltip content={t(label)} preferredPlacement="bottom" key={id}>
          <button
            aria-label={t(label)}
            aria-pressed={selectedView === id}
            className="agent-center__templates agent-center__view-button"
            onClick={() => {
              if (selectedView === id) return
              rememberScroll()
              if (id === 'list') setHomeView('list')
              else {
                setTreeLayout(id)
                setHomeView('tree')
              }
            }}
            type="button"
          >
            <Icon aria-hidden="true" />
          </button>
        </Tooltip>
      ))}
      {onOpenAgentTemplates ? (
        <Tooltip content={t('agentCenter.manageTemplates')} preferredPlacement="bottom">
          <button
            aria-label={t('agentCenter.manageTemplates')}
            className="agent-center__templates"
            onClick={onOpenAgentTemplates}
            type="button"
          >
            <Settings2 aria-hidden="true" />
          </button>
        </Tooltip>
      ) : null}
    </div>
  )

  if (homeView === 'tree') {
    return (
      <div className="agent-center agent-center--tree">
        <div className="agent-center__group agent-center__tree-heading">
          <div className="agent-center__group-heading">
            <h3>{t('agentCenter.treeView')}</h3>
            {headerActions}
          </div>
        </div>
        <AgentTreeView
          layout={treeLayout}
          transmissions={collaborationSnapshot?.transmissions}
          agents={validTree.agents}
          rootAgentId={validTree.rootAgentId}
          userCollapsed={userCollapsed}
          onToggleUser={() => setUserCollapsed((previous) => !previous)}
          onOpenProfile={onOpenProfile}
          collapsedAgentIds={collapsedAgentIds}
          onToggle={(agentId) =>
            setCollapsedAgentIds((previous) => {
              const next = new Set(previous)
              if (next.has(agentId)) next.delete(agentId)
              else next.add(agentId)
              return next
            })
          }
          onOpen={(agentId) => {
            rememberScroll()
            if (agentId === validTree.rootAgentId) {
              onOpenAgentRootConversation?.(rootConversationId)
              return
            }
            onNavigate({ agentId, kind: 'agent-center', rootConversationId, view: 'detail' })
          }}
          registerAvatar={(agentId, element) => {
            if (element) rowElementsRef.current.set(agentId, element)
            else rowElementsRef.current.delete(agentId)
          }}
          scrollRef={treeElementRef}
        />
      </div>
    )
  }

  return (
    <div className="agent-center" ref={listElementRef}>
      <AgentGroup
        agents={active}
        emptyLabel={t('agentCenter.noActive')}
        expanded={showAllActive}
        headerAction={headerActions}
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
            <AgentAvatar agentId={agent.agentId} className="agent-center__avatar" />
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
