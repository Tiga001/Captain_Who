import type { AgentContextWindowSnapshot } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'

interface ContextWindowIndicatorProps {
  snapshot: AgentContextWindowSnapshot
}

function formatTokens(tokens: number, language: string): string {
  const format = (value: number, suffix: string) =>
    `${new Intl.NumberFormat(language, { maximumFractionDigits: 1 }).format(value)}${suffix}`

  if (tokens >= 1_000_000) return format(tokens / 1_000_000, 'M')
  if (tokens >= 1_000) return format(tokens / 1_000, 'K')
  return new Intl.NumberFormat(language).format(tokens)
}

export function ContextWindowIndicator({ snapshot }: ContextWindowIndicatorProps) {
  const { language, t } = useFrontendConfig()
  const totalTokens = snapshot.inputCapacityTokens
  if (!totalTokens || snapshot.status === 'unconfigured') return null

  const rawPercent = (snapshot.inputTokens / totalTokens) * 100
  const progressPercent = Math.min(100, Math.max(0, rawPercent))
  // Keep the visible threshold aligned with backend Compaction: 89.x% must never be presented
  // as 90% before the complete request actually reaches the trigger.
  const usedPercent = Math.floor(progressPercent)
  const remainingPercent = Math.max(0, 100 - usedPercent)
  const level = rawPercent >= 90 ? 'critical' : rawPercent >= 75 ? 'warning' : 'normal'
  const usedTokens = formatTokens(snapshot.inputTokens, language)
  const availableTokens = formatTokens(totalTokens, language)

  return (
    <span className="composer-context-window" data-level={level}>
      <button
        className="composer-context-window__button"
        type="button"
        aria-label={t('chat.contextWindowAria').replace('{used}', String(usedPercent))}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle className="composer-context-window__track" cx="12" cy="12" r="8.5" />
          <circle
            className="composer-context-window__progress"
            cx="12"
            cy="12"
            r="8.5"
            pathLength="100"
            strokeDasharray={`${progressPercent} 100`}
          />
        </svg>
      </button>

      <span className="composer-context-window__popover" role="tooltip">
        <span className="composer-context-window__title">{t('chat.contextWindow')}</span>
        <strong>
          {t('chat.contextUsedSummary')
            .replace('{used}', String(usedPercent))
            .replace('{remaining}', String(remainingPercent))}
        </strong>
        <span>
          {t('chat.contextTokenSummary')
            .replace('{used}', usedTokens)
            .replace('{total}', availableTokens)}
        </span>
      </span>
    </span>
  )
}
