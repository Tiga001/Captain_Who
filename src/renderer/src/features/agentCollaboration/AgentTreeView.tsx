import type { AgentSummary } from '@mycopilot/protocol'
import { Bot, ChevronDown, ChevronRight } from 'lucide-react'
import { useId, useMemo, type ReactNode, type RefObject } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { AccountAvatar } from '../auth/AccountAvatar'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { AgentAvatar } from './AgentAvatar'
import { ACTIVE_STATUSES, baseModelLabel, statusLabel } from './agentCenterLabels'
import type { AgentTreeTransmission } from './agentTreeTransmission'
import { useAgentTreeTransmissions } from './useAgentTreeTransmissions'
import './AgentTreeView.css'

export type AgentTreeLayout = 'diagram' | 'outline'

interface AgentTreeViewProps {
  transmissions?: readonly AgentTreeTransmission[]
  layout: AgentTreeLayout
  agents: readonly AgentSummary[]
  rootAgentId: string
  userCollapsed: boolean
  onToggleUser: () => void
  onOpenProfile?: () => void
  collapsedAgentIds: ReadonlySet<string>
  onToggle: (agentId: string) => void
  onOpen: (agentId: string) => void
  registerAvatar: (agentId: string, element: HTMLButtonElement | null) => void
  scrollRef: RefObject<HTMLDivElement | null>
}

const NO_TRANSMISSIONS: readonly AgentTreeTransmission[] = []

export function AgentTreeView({
  transmissions = NO_TRANSMISSIONS,
  layout,
  agents,
  rootAgentId,
  userCollapsed,
  onToggleUser,
  onOpenProfile,
  collapsedAgentIds,
  onToggle,
  onOpen,
  registerAvatar,
  scrollRef
}: AgentTreeViewProps) {
  const markerId = `agent-transmission-${useId().replace(/:/g, '')}`
  const transmissionView = useAgentTreeTransmissions({
    layout,
    transmissions,
    agents,
    collapsedAgentIds,
    userCollapsed,
    scrollRef
  })
  const { t } = useFrontendConfig()
  const profile = useAccountAuth()?.state.profile
  const userLabel = profile?.displayName?.trim() || profile?.userId || t('auth.signedOut')
  const { roots, childrenByParent } = useMemo(() => {
    const ids = new Set(agents.map((agent) => agent.agentId))
    const roots: AgentSummary[] = []
    const childrenByParent = new Map<string, AgentSummary[]>()
    for (const agent of agents) {
      if (!agent.parentAgentId || !ids.has(agent.parentAgentId)) roots.push(agent)
      else {
        const siblings = childrenByParent.get(agent.parentAgentId) ?? []
        siblings.push(agent)
        childrenByParent.set(agent.parentAgentId, siblings)
      }
    }
    return { roots, childrenByParent }
  }, [agents])

  const renderBranch = (agent: AgentSummary, ancestors: ReadonlySet<string>): ReactNode => {
    if (ancestors.has(agent.agentId)) return null
    const children = childrenByParent.get(agent.agentId) ?? []
    const expanded = !collapsedAgentIds.has(agent.agentId)
    const model = baseModelLabel(agent, t)
    const status = statusLabel(agent.displayStatus, t)
    return (
      <li className="agent-tree__branch" key={agent.agentId}>
        <div
          className="agent-tree__node"
          data-agent-id={agent.agentId}
          data-active={ACTIVE_STATUSES.has(agent.displayStatus)}
          data-status={agent.displayStatus}
        >
          <button
            aria-label={formatTranslation(
              t,
              agent.agentId === rootAgentId ? 'agentCenter.openRootAgent' : 'agentCenter.openAgent',
              {
                name: agent.taskName,
                model,
                status
              }
            )}
            className="agent-tree__avatar-link"
            data-agent-id={agent.agentId}
            onClick={() => onOpen(agent.agentId)}
            ref={(element) => registerAvatar(agent.agentId, element)}
            title={`${agent.taskName} · ${status}`}
            type="button"
          >
            {agent.agentId === rootAgentId ? (
              <span className="agent-tree__avatar agent-tree__avatar--root">
                <Bot aria-hidden="true" />
              </span>
            ) : (
              <AgentAvatar agentId={agent.agentId} className="agent-tree__avatar" />
            )}
          </button>
          <div className="agent-tree__copy">
            <strong title={agent.taskName}>{agent.taskName}</strong>
            <span title={model}>{model}</span>
          </div>
          {children.length > 0 ? (
            <button
              aria-expanded={expanded}
              aria-label={formatTranslation(
                t,
                expanded ? 'agentCenter.collapseAgent' : 'agentCenter.expandAgent',
                { name: agent.taskName }
              )}
              className="agent-tree__toggle"
              onClick={() => onToggle(agent.agentId)}
              type="button"
            >
              {expanded ? <ChevronDown aria-hidden="true" /> : <ChevronRight aria-hidden="true" />}
              <span>{children.length}</span>
            </button>
          ) : null}
        </div>
        {expanded && children.length > 0 ? (
          <ul className="agent-tree__children">
            {children.map((child) => renderBranch(child, new Set([...ancestors, agent.agentId])))}
          </ul>
        ) : null}
      </li>
    )
  }

  return (
    <div
      aria-label={t('agentCenter.treeView')}
      className={`agent-tree agent-tree--${layout}`}
      ref={scrollRef}
      role="region"
      tabIndex={0}
    >
      {roots.length > 0 ? (
        <ul className="agent-tree__roots">
          <li className="agent-tree__branch">
            <div className="agent-tree__node agent-tree__node--user">
              <button
                aria-label={t('settings.page.profile')}
                className="agent-tree__avatar-link"
                disabled={!onOpenProfile}
                onClick={onOpenProfile}
                title={t('settings.page.profile')}
                type="button"
              >
                <span className="agent-avatar agent-tree__avatar">
                  <AccountAvatar src={profile?.avatarDataUrl} />
                </span>
              </button>
              <div className="agent-tree__copy">
                <strong title={userLabel}>{userLabel}</strong>
                <span>Captain Who</span>
              </div>
              <button
                aria-expanded={!userCollapsed}
                aria-label={formatTranslation(
                  t,
                  userCollapsed ? 'agentCenter.expandAgent' : 'agentCenter.collapseAgent',
                  { name: userLabel }
                )}
                className="agent-tree__toggle"
                onClick={onToggleUser}
                type="button"
              >
                {userCollapsed ? (
                  <ChevronRight aria-hidden="true" />
                ) : (
                  <ChevronDown aria-hidden="true" />
                )}
                <span>{roots.length}</span>
              </button>
            </div>
            {!userCollapsed ? (
              <ul className="agent-tree__children">
                {roots.map((agent) => renderBranch(agent, new Set()))}
              </ul>
            ) : null}
          </li>
        </ul>
      ) : (
        <p className="agent-center__empty">{t('agentCenter.noAgents')}</p>
      )}
      {layout === 'diagram' && transmissionView.active.length > 0 ? (
        <svg
          aria-hidden="true"
          className="agent-tree__transmissions"
          width={transmissionView.width}
          height={transmissionView.height}
          focusable="false"
        >
          <defs>
            <marker
              id={markerId}
              markerWidth="5"
              markerHeight="5"
              refX="4"
              refY="2.5"
              orient="auto"
              markerUnits="userSpaceOnUse"
            >
              <path d="M 0 0 L 4 2.5 L 0 5" fill="none" stroke="currentColor" strokeWidth="1.2" />
            </marker>
          </defs>
          {transmissionView.active.map((event) => (
            <g
              key={event.id}
              className="agent-tree__transmission"
              data-transmission-id={event.id}
              data-source-agent-id={event.sourceAgentId ?? 'user'}
              data-target-agent-id={event.targetAgentId ?? 'user'}
              data-route-kind={event.route.kind}
              onAnimationEnd={(animation) => {
                if (animation.target === animation.currentTarget) transmissionView.finish(event.id)
              }}
            >
              <path
                className="agent-tree__transmission-track"
                d={event.route.path}
                markerEnd={`url(#${markerId})`}
              />
              <path
                className="agent-tree__transmission-wave"
                d={event.route.path}
                pathLength="100"
              />
              <path
                className="agent-tree__transmission-head"
                d={event.route.path}
                pathLength="100"
              />
            </g>
          ))}
        </svg>
      ) : null}
    </div>
  )
}
