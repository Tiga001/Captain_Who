import type { AgentSummary } from '@mycopilot/protocol'
import { ConversationSurface } from '../chat/ConversationSurface'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useObserverConversation } from './useObserverConversation'
import type { CollaborationTimelineActivity } from './collaborationTimelineModel'

interface AgentObserverConversationSurfaceProps {
  collaborationTreeAgentIds: readonly string[]
  agent: AgentSummary
  agentLabelsById: Readonly<Record<string, string>>
  activities?: readonly CollaborationTimelineActivity[]
  invalidationVersion: string
  onOpenAgent?: (agentId: string) => void
  rootConversationId: string
  showTokenUsageDetails: boolean
}

export function AgentObserverConversationSurface({
  agent,
  agentLabelsById,
  activities,
  collaborationTreeAgentIds,
  invalidationVersion,
  onOpenAgent,
  rootConversationId,
  showTokenUsageDetails
}: AgentObserverConversationSurfaceProps) {
  const { t } = useFrontendConfig()
  const { conversation, error, loading, reload } = useObserverConversation({
    agentId: agent.agentId,
    rootAgentId: agent.rootAgentId,
    conversationId: agent.conversationId,
    invalidationVersion,
    rootConversationId
  })

  if (loading && !conversation) {
    return (
      <div className="agent-center__state" role="status">
        {t('agentCenter.observerLoading')}
      </div>
    )
  }
  if (!conversation) {
    return (
      <div className="agent-center__state" role="alert">
        <p>{t('agentCenter.observerUnavailable')}</p>
        <button onClick={reload} type="button">
          {t('agentCenter.retry')}
        </button>
      </div>
    )
  }

  return (
    <div className="agent-center__observer-content">
      {error ? (
        <div className="agent-center__state agent-center__state--refresh-error" role="alert">
          <span>{t('agentCenter.observerUnavailable')}</span>
          <button onClick={reload} type="button">
            {t('agentCenter.retry')}
          </button>
        </div>
      ) : null}
      <ConversationSurface
        agentLabelsById={agentLabelsById}
        collaborationTimelineActivities={activities}
        conversation={conversation}
        collaborationTreeAgentIds={collaborationTreeAgentIds}
        mode="observer"
        onOpenCollaborationAgent={onOpenAgent}
        parentAgentId={agent.parentAgentId}
        rootConversationId={rootConversationId}
        showTokenUsageDetails={showTokenUsageDetails}
      />
    </div>
  )
}
