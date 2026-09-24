import type { AgentDisplayStatusView, AgentSummary } from '@mycopilot/protocol'
import type { Translate } from '../../config/translationFormat'

export const ACTIVE_STATUSES = new Set<AgentDisplayStatusView>([
  'queued',
  'running',
  'waiting_approval'
])

export function statusLabel(status: AgentDisplayStatusView, t: Translate): string {
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

export function baseModelLabel(agent: AgentSummary, t: Translate): string {
  const displayName = agent.model?.displayName.trim()
  return displayName || t('agentCenter.modelUnavailable')
}
