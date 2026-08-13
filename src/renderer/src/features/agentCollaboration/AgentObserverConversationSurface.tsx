import type { AgentSummary } from '@mycopilot/protocol'
import { ConversationSurface } from '../chat/ConversationSurface'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useObserverConversation } from './useObserverConversation'

interface AgentObserverConversationSurfaceProps {
  agent: AgentSummary
  agentLabelsById: Readonly<Record<string, string>>
  invalidationVersion: string
  rootConversationId: string
  showTokenUsageDetails: boolean
}

export function AgentObserverConversationSurface({
  agent,
  agentLabelsById,
  invalidationVersion,
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
        conversation={conversation}
        mode="observer"
        parentAgentId={agent.parentAgentId}
        rootConversationId={rootConversationId}
        showTokenUsageDetails={showTokenUsageDetails}
      />
    </div>
  )
}
