import { Search } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface SearchToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export interface SearchToolActivityGroupItem extends SearchToolActivityProps {}

interface SearchToolActivityGroupProps {
  items: SearchToolActivityGroupItem[]
  kind: SearchKind
}

interface SearchMatch {
  lineNumber?: number
  path: string
}

export type SearchKind = 'files' | 'code'
type SearchStatus = 'running' | 'completed' | 'failed' | 'cancelled'

const STATUS_LABELS: Record<SearchKind, Record<SearchStatus, TranslationKey>> = {
  files: {
    running: 'agent.search.files.running',
    completed: 'agent.search.files.completed',
    failed: 'agent.search.files.failed',
    cancelled: 'agent.search.files.cancelled'
  },
  code: {
    running: 'agent.search.code.running',
    completed: 'agent.search.code.completed',
    failed: 'agent.search.code.failed',
    cancelled: 'agent.search.code.cancelled'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function stringValue(value: unknown) {
  return typeof value === 'string' ? value.trim() : ''
}

function numberValue(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

export function isSearchTool(tool: string) {
  return tool === 'search_files' || tool === 'search_code'
}

export function getSearchKind(call: AgentToolCall): SearchKind {
  return call.tool === 'search_code' ? 'code' : 'files'
}

function getStatus(
  cancelled: boolean,
  result: AgentToolResult | undefined,
  settledStatus?: SettledToolStatus
): SearchStatus {
  if (cancelled && !result) return 'cancelled'
  if (result?.ok === false) return 'failed'
  if (result) return 'completed'
  if (settledStatus) return settledStatus
  return 'running'
}

function getMatches(result: AgentToolResult | undefined): SearchMatch[] {
  if (!isRecord(result?.result) || !Array.isArray(result.result.matches)) return []

  return result.result.matches.reduce<SearchMatch[]>((matches, item) => {
    if (!isRecord(item)) return matches
    const path = stringValue(item.path)
    if (!path) return matches
    return [
      ...matches,
      {
        path,
        lineNumber: numberValue(item.lineNumber)
      }
    ]
  }, [])
}

function getQuery(call: AgentToolCall, result: AgentToolResult | undefined) {
  if (isRecord(result?.result)) {
    const resultQuery = stringValue(result.result.query)
    if (resultQuery) return resultQuery
  }

  if (!isRecord(call.args)) return ''
  return stringValue(call.args.query)
}

function getScope(call: AgentToolCall) {
  if (!isRecord(call.args)) return ''
  return stringValue(call.args.path)
}

function getStatusLabel(t: Translate, kind: SearchKind, status: SearchStatus, count: number) {
  const key = STATUS_LABELS[kind][status]
  return status === 'completed' ? formatTranslation(t, key, { count }) : t(key)
}

function SearchResultRow({ kind, match }: { kind: SearchKind; match: SearchMatch }) {
  const { t } = useFrontendConfig()
  const location =
    kind === 'code' && match.lineNumber ? `${match.path}:${match.lineNumber}` : match.path

  return (
    <div className="search-activity__item" title={location}>
      {formatTranslation(t, 'agent.search.item', { location })}
    </div>
  )
}

function getGroupStatus(items: SearchToolActivityGroupItem[]): SearchStatus {
  const statuses = items.map((item) =>
    getStatus(Boolean(item.cancelled && !item.result), item.result, item.settledStatus)
  )
  if (statuses.some((status) => status === 'running')) return 'running'
  if (statuses.every((status) => status === 'cancelled')) return 'cancelled'
  if (statuses.every((status) => status === 'failed')) return 'failed'
  return 'completed'
}

function getGroupLabel(t: Translate, kind: SearchKind, status: SearchStatus, matchCount: number) {
  return getStatusLabel(t, kind, status, matchCount)
}

function groupItemsByQueryAndScope(items: SearchToolActivityGroupItem[]) {
  const groups = new Map<
    string,
    { items: SearchToolActivityGroupItem[]; query: string; scope: string }
  >()

  items.forEach((item) => {
    const query = getQuery(item.call, item.result) || '__empty__'
    const scope = getScope(item.call)
    const key = JSON.stringify([query, scope])
    const group = groups.get(key)
    groups.set(key, {
      items: [...(group?.items ?? []), item],
      query,
      scope
    })
  })

  return [...groups.entries()].map(([key, group]) => ({ key, ...group }))
}

function SearchQuerySection({
  items,
  kind,
  query,
  scope,
  showHeading
}: {
  items: SearchToolActivityGroupItem[]
  kind: SearchKind
  query: string
  scope: string
  showHeading: boolean
}) {
  const { t } = useFrontendConfig()
  const queryLabel = query === '__empty__' ? t('agent.search.unknownQuery') : query
  const matches = items.flatMap((item) => getMatches(item.result))
  const errors = items.map((item) => item.result?.error).filter(Boolean)
  const heading = scope
    ? formatTranslation(t, 'agent.search.queryScope', { query: queryLabel, scope })
    : formatTranslation(t, 'agent.search.query', { query: queryLabel })

  return (
    <section className="search-activity__query-group">
      {showHeading ? <div className="search-activity__query-heading">{heading}</div> : null}
      <div className="search-activity__query-details">
        {errors.map((error, index) => (
          <p className="search-activity__error" key={`${error}:${index}`}>
            {error}
          </p>
        ))}
        {matches.length === 0 && errors.length === 0 ? <p>{t('agent.search.empty')}</p> : null}
        {matches.length > 0 ? (
          <div className="search-activity__items">
            {matches.map((match, index) => (
              <SearchResultRow
                kind={kind}
                key={`${match.path}:${match.lineNumber ?? ''}:${index}`}
                match={match}
              />
            ))}
          </div>
        ) : null}
      </div>
    </section>
  )
}

export function SearchToolActivity({
  cancelled = false,
  call,
  result,
  settledStatus
}: SearchToolActivityProps) {
  const { t } = useFrontendConfig()
  const kind = getSearchKind(call)
  const status = getStatus(cancelled, result, settledStatus)
  const matches = getMatches(result)
  const hasDetails = status !== 'running'
  const error = result?.error

  return (
    <AgentActivityDisclosure
      className="agent-activity--search"
      hasDetails={hasDetails}
      icon={Search}
      isPending={status === 'running'}
      label={getStatusLabel(t, kind, status, matches.length)}
    >
      {hasDetails && (
        <div className="agent-activity__details search-activity__details">
          {error ? <p className="search-activity__error">{error}</p> : null}
          {!error && matches.length === 0 ? <p>{t('agent.search.empty')}</p> : null}
          {!error && matches.length > 0 ? (
            <div className="search-activity__items">
              {matches.map((match, index) => (
                <SearchResultRow
                  kind={kind}
                  key={`${match.path}:${match.lineNumber ?? ''}:${index}`}
                  match={match}
                />
              ))}
            </div>
          ) : null}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}

export function SearchToolActivityGroup({ items, kind }: SearchToolActivityGroupProps) {
  const { t } = useFrontendConfig()

  if (items.length === 0) return null
  if (items.length === 1) {
    const item = items[0]
    return (
      <SearchToolActivity
        cancelled={item.cancelled}
        call={item.call}
        result={item.result}
        settledStatus={item.settledStatus}
      />
    )
  }

  const status = getGroupStatus(items)
  const hasDetails = status !== 'running'
  const queryGroups = groupItemsByQueryAndScope(items)
  const matchCount = items.reduce((count, item) => count + getMatches(item.result).length, 0)

  return (
    <AgentActivityDisclosure
      className="agent-activity--search"
      hasDetails={hasDetails}
      icon={Search}
      isPending={status === 'running'}
      label={getGroupLabel(t, kind, status, matchCount)}
    >
      {hasDetails && (
        <div className="agent-activity__details search-activity__details search-activity__details--group">
          {queryGroups.map((group) => (
            <SearchQuerySection
              items={group.items}
              key={group.key}
              kind={kind}
              query={group.query}
              scope={group.scope}
              showHeading={queryGroups.length > 1}
            />
          ))}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
