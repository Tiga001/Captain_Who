import { useEffect, useMemo, useState } from 'react'
import type { FocusEvent, ReactElement } from 'react'
import { Check, ChevronDown } from 'lucide-react'
import type { AgentUsageModelSummary, AgentUsageSummaryOutput } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useModelSettings } from '../../../config/ModelSettingsProvider'
import type { ModelConfig } from '../../../config/modelConfig'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'
import { clearAgentUsageRecords, getAgentUsageSummary } from '../../agent/agentClient'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import './UsageBillingSettingsPage.css'

interface UsageBillingSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  uiPreferences: UiPreferencesSnapshot
}

const USAGE_RANGE_OPTIONS: Array<{
  labelKey:
    'usageBilling.rangeLast7Days' | 'usageBilling.rangeLast30Days' | 'usageBilling.rangeLastYear'
  value: UsageChartRange
}> = [
  { labelKey: 'usageBilling.rangeLast7Days', value: 'last7Days' },
  { labelKey: 'usageBilling.rangeLast30Days', value: 'last30Days' },
  { labelKey: 'usageBilling.rangeLastYear', value: 'lastYear' }
]

type UsageChartRange = 'last7Days' | 'last30Days' | 'lastYear'
type UsageChartTokenKey =
  'uncachedInputTokens' | 'cachedInputTokens' | 'outputTokens' | 'outputThinkingTokens'

type UsageChartSeries = {
  key: UsageChartTokenKey
  labelKey:
    | 'usageBilling.uncachedInputTokens'
    | 'usageBilling.cachedInputTokens'
    | 'usageBilling.outputTokens'
    | 'usageBilling.outputThinkingTokens'
  className: string
}

type UsageChartBucket = {
  dateLabel: string
  fullDateLabel: string
  from: number
  to: number
  summary: AgentUsageSummaryOutput
}

type UsageChartWindow = Omit<UsageChartBucket, 'summary'>

type UsageValues = Partial<Record<UsageChartTokenKey, number>> & {
  estimatedCost?: number
  unpricedMessageCount?: number
}

const DAY_MS = 24 * 60 * 60 * 1000
const ALL_MODELS_KEY = '__all_models__'
const USAGE_CHART_SERIES: UsageChartSeries[] = [
  {
    key: 'uncachedInputTokens',
    labelKey: 'usageBilling.uncachedInputTokens',
    className: 'usage-chart-bar--input'
  },
  {
    key: 'cachedInputTokens',
    labelKey: 'usageBilling.cachedInputTokens',
    className: 'usage-chart-bar--cached-input'
  },
  {
    key: 'outputTokens',
    labelKey: 'usageBilling.outputTokens',
    className: 'usage-chart-bar--output'
  },
  {
    key: 'outputThinkingTokens',
    labelKey: 'usageBilling.outputThinkingTokens',
    className: 'usage-chart-bar--output-thinking'
  }
]

function formatCount(value: number | undefined, language: string): string {
  if (typeof value !== 'number') return '—'
  return new Intl.NumberFormat(language).format(value)
}

function formatEstimatedCost(value: number | undefined, language: string): string {
  if (typeof value !== 'number') return '—'
  return new Intl.NumberFormat(language, {
    maximumFractionDigits: 6,
    minimumFractionDigits: value > 0 && value < 0.01 ? 6 : 2
  }).format(value)
}

function formatChartEstimatedCost(value: number | undefined, language: string): string {
  if (typeof value !== 'number') return '—'
  return new Intl.NumberFormat(language, {
    maximumFractionDigits: value > 0 && value < 0.01 ? 4 : 2
  }).format(value)
}

function formatTokenCount(value: number | undefined, language: string, tokenLabel: string): string {
  if (typeof value !== 'number') return `— ${tokenLabel}`
  return `${formatCount(value, language)} ${tokenLabel}`
}

function formatDateLabel(timestamp: number, language: string): string {
  return new Intl.DateTimeFormat(language, {
    month: '2-digit',
    day: '2-digit'
  }).format(new Date(timestamp))
}

function formatFullDateLabel(timestamp: number, language: string): string {
  return new Intl.DateTimeFormat(language, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  }).format(new Date(timestamp))
}

function formatMonthLabel(timestamp: number): string {
  const date = new Date(timestamp)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`
}

function getModelKey(model: AgentUsageModelSummary): string {
  return model.modelId
}

interface UsageModelPresentation {
  label: string
  subtitle: string
}

function getModelPresentation(
  model: AgentUsageModelSummary,
  configuredModel: ModelConfig | undefined
): UsageModelPresentation {
  const label = configuredModel?.displayName.trim() || model.modelName.trim() || '—'
  return {
    label,
    subtitle: ''
  }
}

function getModelLabel(
  model: AgentUsageModelSummary,
  configuredModel: ModelConfig | undefined,
  deletedLabel: string
): string {
  const label = configuredModel
    ? formatModelConfigLabel(configuredModel)
    : getModelPresentation(model, configuredModel).label
  return model.isConfigured ? label : `${label} (${deletedLabel})`
}

function getRangeDayCount(range: UsageChartRange): number {
  return range === 'last7Days' ? 7 : 30
}

function startOfLocalDay(timestamp: number): number {
  const date = new Date(timestamp)
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
}

function startOfLocalMonth(timestamp: number): number {
  const date = new Date(timestamp)
  return new Date(date.getFullYear(), date.getMonth(), 1).getTime()
}

function addLocalMonths(timestamp: number, monthDelta: number): number {
  const date = new Date(timestamp)
  return new Date(date.getFullYear(), date.getMonth() + monthDelta, 1).getTime()
}

function getUsageChartWindows(range: UsageChartRange, language: string): UsageChartWindow[] {
  if (range === 'lastYear') {
    const currentMonthStart = startOfLocalMonth(Date.now())
    const firstMonthStart = addLocalMonths(currentMonthStart, -11)

    return Array.from({ length: 12 }, (_, index) => {
      const from = addLocalMonths(firstMonthStart, index)
      const nextMonth = addLocalMonths(from, 1)
      const monthLabel = formatMonthLabel(from)
      return {
        from,
        to: nextMonth - 1,
        dateLabel: monthLabel,
        fullDateLabel: monthLabel
      }
    })
  }

  const dayCount = getRangeDayCount(range)
  const todayStart = startOfLocalDay(Date.now())
  const firstDayStart = todayStart - (dayCount - 1) * DAY_MS
  return Array.from({ length: dayCount }, (_, index) => {
    const from = firstDayStart + index * DAY_MS
    return {
      from,
      to: from + DAY_MS - 1,
      dateLabel: formatDateLabel(from, language),
      fullDateLabel: formatFullDateLabel(from, language)
    }
  })
}

function getChartRangeSummaryInput(buckets: Array<Pick<UsageChartBucket, 'from' | 'to'>>): {
  range: 'custom'
  from?: number
  to?: number
} {
  const firstBucket = buckets[0]
  const lastBucket = buckets[buckets.length - 1]
  return { range: 'custom', from: firstBucket?.from, to: lastBucket?.to }
}

async function loadUsageChartBuckets(
  range: UsageChartRange,
  language: string
): Promise<UsageChartBucket[]> {
  const windows = getUsageChartWindows(range, language)
  const summaries = await Promise.all(
    windows.map((window) =>
      getAgentUsageSummary({ range: 'custom', from: window.from, to: window.to })
    )
  )

  return windows.map((window, index) => ({
    ...window,
    summary: summaries[index]
  }))
}

function getNiceChartMax(value: number): number {
  if (value <= 0) return 0
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const normalized = value / magnitude
  const niceNormalized = normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10
  return niceNormalized * magnitude
}

function getUsageValues(
  source: AgentUsageSummaryOutput | AgentUsageModelSummary | undefined
): UsageValues {
  const cachedInputTokens =
    typeof source?.inputTokens === 'number' && typeof source.cachedInputTokens === 'number'
      ? Math.min(source.cachedInputTokens, source.inputTokens)
      : undefined
  const uncachedInputTokens =
    typeof source?.inputTokens === 'number'
      ? Math.max(0, source.inputTokens - (cachedInputTokens ?? 0))
      : undefined

  return {
    uncachedInputTokens,
    cachedInputTokens,
    outputTokens: source?.outputTokens,
    outputThinkingTokens: source?.outputThinkingTokens,
    estimatedCost: source?.estimatedCost,
    unpricedMessageCount: source?.unpricedMessageCount
  }
}

export function UsageBillingSettingsPage({
  onUiPreferencesChange,
  uiPreferences
}: UsageBillingSettingsPageProps): ReactElement {
  const { language, t } = useFrontendConfig()
  const { models } = useModelSettings()
  const [range, setRange] = useState<UsageChartRange>('last7Days')
  const [summary, setSummary] = useState<AgentUsageSummaryOutput | null>(null)
  const [chartBuckets, setChartBuckets] = useState<UsageChartBucket[]>([])
  const [selectedModelKey, setSelectedModelKey] = useState(ALL_MODELS_KEY)
  const [isModelMenuOpen, setModelMenuOpen] = useState(false)
  const [isLoading, setIsLoading] = useState(true)
  const [isClearing, setIsClearing] = useState(false)
  const [isClearDialogOpen, setClearDialogOpen] = useState(false)
  const [statusMessage, setStatusMessage] = useState('')
  const [errorMessage, setErrorMessage] = useState('')
  const sortedModels = useMemo(
    () =>
      [...(summary?.models ?? [])].sort(
        (first, second) => second.requestCount - first.requestCount
      ),
    [summary?.models]
  )
  const configuredModelsById = useMemo(
    () => new Map(models.map((model) => [model.id, model])),
    [models]
  )
  const activeModelKey =
    selectedModelKey === ALL_MODELS_KEY ||
    sortedModels.some((model) => getModelKey(model) === selectedModelKey)
      ? selectedModelKey
      : ALL_MODELS_KEY
  const selectedModel = useMemo(
    () => sortedModels.find((model) => getModelKey(model) === activeModelKey),
    [activeModelKey, sortedModels]
  )
  const visibleSummary = activeModelKey === ALL_MODELS_KEY ? summary : (selectedModel ?? null)
  const chartData = useMemo(
    () =>
      chartBuckets.map((chartBucket) => {
        const source =
          activeModelKey === ALL_MODELS_KEY
            ? chartBucket.summary
            : chartBucket.summary.models.find((model) => getModelKey(model) === activeModelKey)

        return {
          dateLabel: chartBucket.dateLabel,
          fullDateLabel: chartBucket.fullDateLabel,
          values: getUsageValues(source)
        }
      }),
    [activeModelKey, chartBuckets]
  )
  const chartMaxValue = useMemo(() => {
    const maxValue = chartData.reduce((currentMax, day) => {
      const dayMax = USAGE_CHART_SERIES.reduce((seriesMax, series) => {
        const value = day.values[series.key]
        return typeof value === 'number' ? Math.max(seriesMax, value) : seriesMax
      }, 0)
      return Math.max(currentMax, dayMax)
    }, 0)

    return getNiceChartMax(maxValue)
  }, [chartData])
  const chartTicks = useMemo(
    () => [chartMaxValue, chartMaxValue * 0.75, chartMaxValue * 0.5, chartMaxValue * 0.25, 0],
    [chartMaxValue]
  )
  const hasChartData = chartMaxValue > 0

  useEffect(() => {
    let isCancelled = false

    async function loadUsageData(): Promise<void> {
      setIsLoading(true)
      setErrorMessage('')
      try {
        const nextChartBuckets = await loadUsageChartBuckets(range, language)
        const nextSummary = await getAgentUsageSummary(getChartRangeSummaryInput(nextChartBuckets))
        if (isCancelled) return
        setSummary(nextSummary)
        setChartBuckets(nextChartBuckets)
      } catch (error) {
        if (!isCancelled) {
          setSummary(null)
          setChartBuckets([])
          setErrorMessage(getUserFacingErrorMessage(error, t, 'usageBilling.loadFailed'))
        }
      } finally {
        if (!isCancelled) setIsLoading(false)
      }
    }

    void loadUsageData()

    return () => {
      isCancelled = true
    }
  }, [language, range, t])

  const closeModelMenuOnBlur = (event: FocusEvent<HTMLDivElement>): void => {
    if (!event.currentTarget.contains(event.relatedTarget)) {
      setModelMenuOpen(false)
    }
  }

  const clearAllUsageRecords = async (): Promise<void> => {
    setIsClearing(true)
    setErrorMessage('')
    setStatusMessage('')
    try {
      const output = await clearAgentUsageRecords({})
      setClearDialogOpen(false)
      setStatusMessage(
        `${t('usageBilling.clearCompleted')} ${formatCount(output.deletedRecords, language)}`
      )
      const nextChartBuckets = await loadUsageChartBuckets(range, language)
      const nextSummary = await getAgentUsageSummary(getChartRangeSummaryInput(nextChartBuckets))
      setSummary(nextSummary)
      setChartBuckets(nextChartBuckets)
    } catch (error) {
      setErrorMessage(getUserFacingErrorMessage(error, t, 'usageBilling.clearFailed'))
    } finally {
      setIsClearing(false)
    }
  }

  return (
    <article className="settings-list-page usage-billing-settings-page">
      <h1>{t('settings.page.usageBilling')}</h1>

      <section
        className="settings-list-section usage-summary-section"
        aria-labelledby="usage-summary-heading"
      >
        <div className="usage-summary-header">
          <h2 id="usage-summary-heading">{t('usageBilling.summary')}</h2>

          <div className="usage-summary-actions">
            <button
              className="secondary-settings-button usage-clear-button"
              type="button"
              onClick={() => setClearDialogOpen(true)}
            >
              {t('usageBilling.clear')}
            </button>
          </div>
        </div>

        <div className="usage-chart-controls">
          <div className="usage-chart-filter">
            <span>{t('usageBilling.usageTime')}:</span>
            <div className="usage-range-control" aria-label={t('usageBilling.range')}>
              {USAGE_RANGE_OPTIONS.map((option) => (
                <button
                  data-active={range === option.value || undefined}
                  type="button"
                  key={option.value}
                  onClick={() => {
                    setRange(option.value)
                    setStatusMessage('')
                  }}
                >
                  {t(option.labelKey)}
                </button>
              ))}
            </div>
          </div>

          <div className="usage-chart-filter usage-chart-filter--model">
            <span>{t('usageBilling.model')}:</span>
            <div className="usage-model-filter-control" onBlur={closeModelMenuOnBlur}>
              <button
                className="usage-model-filter-button"
                type="button"
                aria-haspopup="listbox"
                aria-expanded={isModelMenuOpen}
                onClick={() => setModelMenuOpen((current) => !current)}
              >
                <span>
                  {selectedModel
                    ? getModelLabel(
                        selectedModel,
                        configuredModelsById.get(selectedModel.modelId),
                        t('usageBilling.deletedModel')
                      )
                    : t('usageBilling.allModels')}
                </span>
                <ChevronDown aria-hidden="true" />
              </button>

              {isModelMenuOpen && (
                <div
                  className="usage-model-filter-menu"
                  role="listbox"
                  aria-label={t('usageBilling.model')}
                >
                  <button
                    className="usage-model-filter-option"
                    data-selected={activeModelKey === ALL_MODELS_KEY || undefined}
                    type="button"
                    role="option"
                    aria-selected={activeModelKey === ALL_MODELS_KEY}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={() => {
                      setSelectedModelKey(ALL_MODELS_KEY)
                      setModelMenuOpen(false)
                    }}
                  >
                    <span>{t('usageBilling.allModels')}</span>
                    {activeModelKey === ALL_MODELS_KEY && <Check aria-hidden="true" />}
                  </button>

                  {sortedModels.map((model) => {
                    const modelKey = getModelKey(model)
                    const isSelected = activeModelKey === modelKey
                    return (
                      <button
                        className="usage-model-filter-option"
                        data-selected={isSelected || undefined}
                        type="button"
                        role="option"
                        aria-selected={isSelected}
                        key={modelKey}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => {
                          setSelectedModelKey(modelKey)
                          setModelMenuOpen(false)
                        }}
                      >
                        <span>
                          {getModelLabel(
                            model,
                            configuredModelsById.get(model.modelId),
                            t('usageBilling.deletedModel')
                          )}
                        </span>
                        {isSelected && <Check aria-hidden="true" />}
                      </button>
                    )
                  })}
                </div>
              )}
            </div>
          </div>
        </div>

        <div className="usage-chart-panel" aria-busy={isLoading}>
          <div className="usage-chart-legend" aria-label={t('usageBilling.chartLegend')}>
            {USAGE_CHART_SERIES.map((series) => (
              <span key={series.key}>
                <i className={series.className} aria-hidden="true" />
                {t(series.labelKey)}
              </span>
            ))}
          </div>

          {isLoading ? (
            <div className="usage-chart-empty">{t('usageBilling.loading')}</div>
          ) : hasChartData ? (
            <div className="usage-chart-scroller">
              <div
                className="usage-chart"
                style={{ minWidth: `${Math.max(chartData.length * 96, 720)}px` }}
              >
                <div className="usage-chart-y-axis" aria-hidden="true">
                  {chartTicks.map((tick, index) => (
                    <span key={`${tick}-${index}`}>{formatCount(Math.round(tick), language)}</span>
                  ))}
                </div>

                <div className="usage-chart-plot">
                  {chartTicks.map((tick, index) => (
                    <span
                      className="usage-chart-grid-line"
                      key={`${tick}-${index}`}
                      aria-hidden="true"
                    />
                  ))}

                  <div className="usage-chart-bars">
                    {chartData.map((day) => {
                      const unpricedMessageCount = day.values.unpricedMessageCount ?? 0
                      const completenessMarker = unpricedMessageCount > 0 ? '*' : ''
                      const costLabel = `${formatEstimatedCost(day.values.estimatedCost, language)}${completenessMarker}`
                      const chartCostLabel = `${formatChartEstimatedCost(day.values.estimatedCost, language)}${completenessMarker}`
                      const dayMaxValue = USAGE_CHART_SERIES.reduce((currentMax, series) => {
                        const value = day.values[series.key]
                        return typeof value === 'number' ? Math.max(currentMax, value) : currentMax
                      }, 0)
                      const dayMaxRatio =
                        chartMaxValue > 0 && dayMaxValue > 0 ? dayMaxValue / chartMaxValue : 0
                      const costBottom =
                        dayMaxRatio > 0 ? `${Math.min(dayMaxRatio * 100 + 2, 92)}%` : '8px'
                      const ariaLabel = [
                        day.fullDateLabel,
                        `${t('usageBilling.estimatedCost')} ${costLabel}`,
                        `${t('usageBilling.uncachedInputTokens')} ${formatTokenCount(day.values.uncachedInputTokens, language, t('usageBilling.tokens'))}`,
                        `${t('usageBilling.cachedInputTokens')} ${formatTokenCount(day.values.cachedInputTokens, language, t('usageBilling.tokens'))}`,
                        `${t('usageBilling.outputTokens')} ${formatTokenCount(day.values.outputTokens, language, t('usageBilling.tokens'))}`,
                        `${t('usageBilling.outputThinkingTokens')} ${formatTokenCount(day.values.outputThinkingTokens, language, t('usageBilling.tokens'))}`
                      ].join(', ')

                      return (
                        <div
                          className="usage-chart-day"
                          key={day.fullDateLabel}
                          tabIndex={0}
                          aria-label={ariaLabel}
                        >
                          <div className="usage-chart-day__bars">
                            {USAGE_CHART_SERIES.map((series) => {
                              const value = day.values[series.key]
                              const ratio =
                                chartMaxValue > 0 && typeof value === 'number' && value > 0
                                  ? value / chartMaxValue
                                  : 0
                              const height = ratio > 0 ? `${Math.max(ratio * 100, 2)}%` : '0%'

                              return (
                                <span
                                  className={`usage-chart-bar ${series.className}`}
                                  style={{ height }}
                                  key={series.key}
                                />
                              )
                            })}
                            <span className="usage-chart-cost" style={{ bottom: costBottom }}>
                              {chartCostLabel}
                            </span>
                          </div>

                          <div className="usage-chart-tooltip" role="tooltip">
                            <strong className="usage-chart-tooltip__date">
                              {day.fullDateLabel}
                            </strong>
                            <span className="usage-chart-tooltip__row">
                              <span>{t('usageBilling.estimatedCost')}</span>
                              <strong>{costLabel}</strong>
                            </span>
                            {unpricedMessageCount > 0 && (
                              <span className="usage-chart-tooltip__warning">
                                {formatCount(unpricedMessageCount, language)}{' '}
                                {t('usageBilling.unpricedMessages')}
                              </span>
                            )}

                            <span className="usage-chart-tooltip__row">
                              <span>
                                <i className="usage-chart-bar--input" aria-hidden="true" />
                                {t('usageBilling.uncachedInputTokens')}
                              </span>
                              <strong>
                                {formatTokenCount(
                                  day.values.uncachedInputTokens,
                                  language,
                                  t('usageBilling.tokens')
                                )}
                              </strong>
                            </span>
                            <span className="usage-chart-tooltip__row">
                              <span>
                                <i className="usage-chart-bar--cached-input" aria-hidden="true" />
                                {t('usageBilling.cachedInputTokens')}
                              </span>
                              <strong>
                                {formatTokenCount(
                                  day.values.cachedInputTokens,
                                  language,
                                  t('usageBilling.tokens')
                                )}
                              </strong>
                            </span>
                            <span className="usage-chart-tooltip__row">
                              <span>
                                <i className="usage-chart-bar--output" aria-hidden="true" />
                                {t('usageBilling.outputTokens')}
                              </span>
                              <strong>
                                {formatTokenCount(
                                  day.values.outputTokens,
                                  language,
                                  t('usageBilling.tokens')
                                )}
                              </strong>
                            </span>
                            <span className="usage-chart-tooltip__row">
                              <span>
                                <i
                                  className="usage-chart-bar--output-thinking"
                                  aria-hidden="true"
                                />
                                {t('usageBilling.outputThinkingTokens')}
                              </span>
                              <strong>
                                {formatTokenCount(
                                  day.values.outputThinkingTokens,
                                  language,
                                  t('usageBilling.tokens')
                                )}
                              </strong>
                            </span>
                          </div>

                          <span className="usage-chart-day__label" title={day.fullDateLabel}>
                            {day.dateLabel}
                          </span>
                        </div>
                      )
                    })}
                  </div>
                </div>
              </div>
            </div>
          ) : (
            <div className="usage-chart-empty">{t('usageBilling.chartEmpty')}</div>
          )}
        </div>

        <div className="usage-summary-grid" aria-busy={isLoading}>
          <div className="usage-summary-card">
            <span>{t('usageBilling.requestCount')}</span>
            <strong>{formatCount(visibleSummary?.requestCount, language)}</strong>
          </div>
          <div className="usage-summary-card">
            <span>{t('usageBilling.messageCount')}</span>
            <strong>{formatCount(visibleSummary?.messageCount, language)}</strong>
          </div>
          <div className="usage-summary-card">
            <span>{t('usageBilling.inputTokens')}</span>
            <strong>{formatCount(visibleSummary?.inputTokens, language)}</strong>
          </div>
          <div className="usage-summary-card">
            <span>{t('usageBilling.outputTokens')}</span>
            <strong>{formatCount(visibleSummary?.outputTokens, language)}</strong>
          </div>
          <div className="usage-summary-card">
            <span>{t('usageBilling.totalTokens')}</span>
            <strong>{formatCount(visibleSummary?.totalTokens, language)}</strong>
          </div>
          <div className="usage-summary-card">
            <span>{t('usageBilling.estimatedCost')}</span>
            <strong>{formatEstimatedCost(visibleSummary?.estimatedCost, language)}</strong>
            {(visibleSummary?.unpricedMessageCount ?? 0) > 0 && (
              <small className="usage-cost-warning">
                {formatCount(visibleSummary?.unpricedMessageCount, language)}{' '}
                {t('usageBilling.unpricedMessages')}
              </small>
            )}
          </div>
        </div>

        <div className="usage-secondary-grid">
          <div className="usage-secondary-stat">
            <span>{t('usageBilling.cachedInputTokens')}</span>
            <strong>{formatCount(visibleSummary?.cachedInputTokens, language)}</strong>
          </div>
          <div className="usage-secondary-stat">
            <span>{t('usageBilling.outputThinkingTokens')}</span>
            <strong>{formatCount(visibleSummary?.outputThinkingTokens, language)}</strong>
          </div>
        </div>

        {statusMessage && <p className="usage-status-message">{statusMessage}</p>}
        {errorMessage && <p className="usage-error-message">{errorMessage}</p>}

        <div className="usage-models-panel">
          <h2>{t('usageBilling.models')}</h2>

          {sortedModels.length > 0 ? (
            <div className="usage-model-list">
              {sortedModels.map((model) => {
                const presentation = getModelPresentation(
                  model,
                  configuredModelsById.get(model.modelId)
                )
                return (
                  <div className="usage-model-row" key={model.modelId}>
                    <div className="usage-model-row__name">
                      <strong>{presentation.label}</strong>
                      {presentation.subtitle && <span>{presentation.subtitle}</span>}
                      {!model.isConfigured && (
                        <small className="usage-model-row__deleted">
                          {t('usageBilling.deletedModel')}
                        </small>
                      )}
                    </div>
                    <div className="usage-model-row__metrics">
                      <span>
                        {t('usageBilling.requestCount')} {formatCount(model.requestCount, language)}
                      </span>
                      <span>
                        {t('usageBilling.messageCount')} {formatCount(model.messageCount, language)}
                      </span>
                      <span>
                        {t('usageBilling.totalTokens')} {formatCount(model.totalTokens, language)}
                      </span>
                      <span className="usage-model-row__cost">
                        {t('usageBilling.estimatedCost')}{' '}
                        {formatEstimatedCost(model.estimatedCost, language)}
                        {model.unpricedMessageCount > 0 && (
                          <small className="usage-cost-warning">
                            {formatCount(model.unpricedMessageCount, language)}{' '}
                            {t('usageBilling.unpricedMessages')}
                          </small>
                        )}
                      </span>
                    </div>
                  </div>
                )
              })}
            </div>
          ) : (
            <div className="usage-empty-state">
              {isLoading ? t('usageBilling.loading') : t('usageBilling.empty')}
            </div>
          )}
        </div>
      </section>

      <section
        className="settings-list-section usage-display-section"
        aria-labelledby="usage-display-heading"
      >
        <div className="settings-list-section__header">
          <h2 id="usage-display-heading">{t('usageBilling.displaySettings')}</h2>
        </div>

        <div className="settings-list">
          <div className="settings-list-row">
            <div className="settings-list-row__text">
              <h3 className="settings-list-row__title" id="usage-token-details-heading">
                {t('usageBilling.tokenDetails')}
              </h3>
              <p className="settings-list-row__description">
                {t('usageBilling.tokenDetailsDescription')}
              </p>
            </div>

            <button
              className="settings-switch"
              type="button"
              role="switch"
              aria-checked={uiPreferences.showTokenUsageDetails}
              data-state={uiPreferences.showTokenUsageDetails ? 'on' : 'off'}
              onClick={() =>
                onUiPreferencesChange({
                  showTokenUsageDetails: !uiPreferences.showTokenUsageDetails
                })
              }
            >
              <span className="sr-only">{t('usageBilling.tokenDetails')}</span>
              <span className="settings-switch__thumb" aria-hidden="true" />
            </button>
          </div>
        </div>
      </section>

      {isClearDialogOpen && (
        <div
          className="usage-clear-dialog"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="usage-clear-dialog-title"
        >
          <div className="usage-clear-dialog__card">
            <h2 id="usage-clear-dialog-title">{t('usageBilling.clearTitle')}</h2>
            <p>{t('usageBilling.clearDescription')}</p>
            <div className="usage-clear-dialog__actions">
              <button
                className="secondary-settings-button"
                type="button"
                onClick={() => setClearDialogOpen(false)}
              >
                {t('usageBilling.cancel')}
              </button>
              <button
                className="danger-settings-button"
                type="button"
                disabled={isClearing}
                onClick={clearAllUsageRecords}
              >
                {t('usageBilling.clearConfirm')}
              </button>
            </div>
          </div>
        </div>
      )}
    </article>
  )
}
