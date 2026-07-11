// Renderer UI.
import { Globe2 } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
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

export interface WebSearchToolActivityGroupItem extends WebSearchToolActivityProps {}

interface WebSearchToolActivityGroupProps {
  items: WebSearchToolActivityGroupItem[]
}

type WebActivityStatus = 'running' | 'completed' | 'failed' | 'cancelled'
type WebActivityKind = NonNullable<ChatWebSearchActivity['kind']>

const GROUP_LABELS: Record<
  WebActivityKind,
  Record<
    'running' | 'completed' | 'processed' | 'completedCount' | 'failedCount' | 'cancelledCount',
    TranslationKey
  >
> = {
  search: {
    running: 'agent.web.search.groupRunning',
    completed: 'agent.web.search.groupCompleted',
    processed: 'agent.web.search.groupProcessed',
    completedCount: 'agent.web.search.groupCompletedCount',
    failedCount: 'agent.web.search.groupFailedCount',
    cancelledCount: 'agent.web.search.groupCancelledCount'
  },
  fetch: {
    running: 'agent.web.fetch.groupRunning',
    completed: 'agent.web.fetch.groupCompleted',
    processed: 'agent.web.fetch.groupProcessed',
    completedCount: 'agent.web.fetch.groupCompletedCount',
    failedCount: 'agent.web.fetch.groupFailedCount',
    cancelledCount: 'agent.web.fetch.groupCancelledCount'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function getWebActivityKind(
  call: AgentToolCall,
  activity: ChatWebSearchActivity | undefined
): WebActivityKind {
  return activity?.kind ?? (call.tool === 'web_fetch' ? 'fetch' : 'search')
}

function getWebActivityQuery(
  call: AgentToolCall,
  activity: ChatWebSearchActivity | undefined
): string {
  const activityQuery = activity?.query.trim()
  if (activityQuery) return activityQuery
  if (!isRecord(call.args)) return ''
  return stringValue(call.tool === 'web_fetch' ? call.args.url : call.args.query)
}

function getWebActivityStatusLabel(
  t: Translate,
  kind: ChatWebSearchActivity['kind'],
  status: WebActivityStatus
): string {
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
): WebActivityStatus {
  if (result?.ok === false) return 'failed'
  if (result) return 'completed'
  if (activity?.status && activity.status !== 'running') return activity.status
  if (settledStatus) return settledStatus
  return activity?.status ?? 'running'
}

function getWebActivityLabel(
  t: Translate,
  call: AgentToolCall,
  activity: ChatWebSearchActivity | undefined,
  status: WebActivityStatus
): string {
  const kind = getWebActivityKind(call, activity)
  const statusLabel = getWebActivityStatusLabel(t, kind, status)
  const subject =
    kind === 'fetch' ? activity?.sources[0]?.title.trim() : getWebActivityQuery(call, activity)
  const fallbackSubject = subject || getWebActivityQuery(call, activity)
  if (!fallbackSubject) return statusLabel
  return formatTranslation(t, 'agent.web.statusWithQuery', {
    status: statusLabel,
    query: fallbackSubject
  })
}

function getWebActivityGroupLabel(
  t: Translate,
  kind: WebActivityKind,
  items: WebSearchToolActivityGroupItem[]
): string {
  const labels = GROUP_LABELS[kind]
  const counts = items.reduce(
    (current, item) => {
      current[getWebActivityStatus(item.activity, item.result, item.settledStatus)] += 1
      return current
    },
    { cancelled: 0, completed: 0, failed: 0, running: 0 }
  )

  if (counts.running > 0) {
    const progress = [
      counts.completed > 0
        ? formatTranslation(t, labels.completedCount, {
            count: counts.completed
          })
        : '',
      counts.failed > 0 ? formatTranslation(t, labels.failedCount, { count: counts.failed }) : '',
      counts.cancelled > 0
        ? formatTranslation(t, labels.cancelledCount, {
            count: counts.cancelled
          })
        : ''
    ].filter(Boolean)

    return [t(labels.running), ...progress].join(t('agent.separator'))
  }

  if (counts.failed === 0 && counts.cancelled === 0) {
    return formatTranslation(t, labels.completed, { count: items.length })
  }

  const results = [
    counts.completed > 0
      ? formatTranslation(t, labels.completedCount, {
          count: counts.completed
        })
      : '',
    counts.failed > 0 ? formatTranslation(t, labels.failedCount, { count: counts.failed }) : '',
    counts.cancelled > 0
      ? formatTranslation(t, labels.cancelledCount, {
          count: counts.cancelled
        })
      : ''
  ].filter(Boolean)

  return [formatTranslation(t, labels.processed, { count: items.length }), ...results].join(
    t('agent.separator')
  )
}

export function WebSearchToolActivity({
  activity,
  call,
  result,
  settledStatus
}: WebSearchToolActivityProps): React.JSX.Element {
  const { t } = useFrontendConfig()
  const status = getWebActivityStatus(activity, result, settledStatus)
  const kind = getWebActivityKind(call, activity)
  const hasLowQualitySummary = kind === 'fetch' && activity?.summaryQuality === 'low'
  const summary = hasLowQualitySummary ? t('agent.web.fetch.lowQuality') : activity?.answer
  const hasDetails = Boolean(
    activity?.sources.length || summary || activity?.error || result?.error
  )
  const label = getWebActivityLabel(t, call, activity, status)
  const isPending = status === 'running'

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
          {activity?.error || result?.error ? (
            <>
              <span>{t('agent.detail.error')}</span>
              <pre>{activity?.error ?? result?.error}</pre>
            </>
          ) : null}
          {Boolean(activity?.sources.length) && (
            <WebSearchSourcesList sources={activity?.sources ?? []} />
          )}
          {summary && (
            <p
              className="web-search-activity__answer"
              data-quality={hasLowQualitySummary ? 'low' : 'good'}
            >
              <span className="web-search-activity__answer-label">{t('agent.detail.summary')}</span>
              <span className="web-search-activity__answer-text">{summary}</span>
            </p>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}

export function WebSearchToolActivityGroup({
  items
}: WebSearchToolActivityGroupProps): React.JSX.Element | null {
  const { t } = useFrontendConfig()
  const firstItem = items[0]
  if (!firstItem) return null
  if (items.length === 1) {
    return (
      <WebSearchToolActivity
        activity={firstItem.activity}
        call={firstItem.call}
        result={firstItem.result}
        settledStatus={firstItem.settledStatus}
      />
    )
  }

  const isPending = items.some(
    (item) => getWebActivityStatus(item.activity, item.result, item.settledStatus) === 'running'
  )
  const kind = getWebActivityKind(firstItem.call, firstItem.activity)

  return (
    <AgentActivityDisclosure
      className="agent-activity--web-search agent-activity--web-search-group"
      hasDetails
      icon={Globe2}
      isPending={isPending}
      label={getWebActivityGroupLabel(t, kind, items)}
    >
      <div className="web-search-activity__group-items">
        {items.map((item) => (
          <WebSearchToolActivity
            activity={item.activity}
            call={item.call}
            key={item.call.id}
            result={item.result}
            settledStatus={item.settledStatus}
          />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
