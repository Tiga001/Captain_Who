// Renderer UI.
import { Globe2 } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { Translate } from '../../../../config/translationFormat'
import type { ChatWebSearchActivity } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'
import { WebSearchSourcesList } from './WebSearchSources'

interface WebSearchToolActivityProps {
  activity?: ChatWebSearchActivity
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

function getWebActivityKind(call: AgentToolCall, activity: ChatWebSearchActivity | undefined) {
  return activity?.kind ?? (call.tool === 'web_fetch' ? 'fetch' : 'search')
}

function getWebActivityStatusLabel(
  t: Translate,
  kind: ChatWebSearchActivity['kind'],
  status: 'running' | 'completed' | 'failed' | 'cancelled'
) {
  if (kind === 'fetch') {
    if (status === 'cancelled') return t('agent.web.fetch.cancelled')
    if (status === 'failed') return t('agent.web.fetch.failed')
    if (status === 'completed') return t('agent.web.fetch.completed')
    return t('agent.web.fetch.running')
  }

  if (status === 'cancelled') return t('agent.web.search.cancelled')
  if (status === 'failed') return t('agent.web.search.failed')
  if (status === 'completed') return t('agent.web.search.completed')
  return t('agent.web.search.running')
}

function getWebActivityStatus(
  activity: ChatWebSearchActivity | undefined,
  result: AgentToolResult | undefined,
  settledStatus?: SettledToolStatus
) {
  if (result?.ok === false) return 'failed'
  if (result) return 'completed'
  if (activity?.status && activity.status !== 'running') return activity.status
  if (settledStatus) return settledStatus
  return activity?.status ?? 'running'
}

export function WebSearchToolActivity({
  activity,
  call,
  result,
  settledStatus
}: WebSearchToolActivityProps) {
  const { t } = useFrontendConfig()
  const kind = getWebActivityKind(call, activity)
  const status = getWebActivityStatus(activity, result, settledStatus)
  const hasDetails = Boolean(
    activity?.query ||
    activity?.sources.length ||
    activity?.answer ||
    activity?.error ||
    result?.error
  )
  const label = getWebActivityStatusLabel(t, kind, status)
  const isPending = status === 'running'
  const queryLabel = kind === 'fetch' ? t('agent.web.query.fetch') : t('agent.web.query.search')

  return (
    <AgentActivityDisclosure
      className="agent-activity--web-search"
      hasDetails={hasDetails}
      icon={Globe2}
      isPending={isPending}
      label={label}
    >
      {hasDetails && (
        <div className="agent-activity__details web-search-activity__details">
          {activity?.query && (
            <p className="web-search-activity__query">
              <span>{queryLabel}</span>
              {activity.query}
            </p>
          )}
          {activity?.error || result?.error ? (
            <>
              <span>{t('agent.detail.error')}</span>
              <pre>{activity?.error ?? result?.error}</pre>
            </>
          ) : null}
          {activity?.sources && <WebSearchSourcesList sources={activity.sources} />}
          {activity?.answer && (
            <p className="web-search-activity__answer">
              <span>{t('agent.detail.summary')}</span>
              {activity.answer}
            </p>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
