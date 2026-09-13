import { useEffect, useMemo, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, RefreshCw } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { hostClient } from '../../../host/hostClient'
import {
  buildTokenYear,
  parseTokenCount,
  shanghaiDate,
  tokenIntensity
} from './localTokenActivityData'
import './LocalTokenActivity.css'

type Usage = Awaited<ReturnType<typeof hostClient.agent.getLocalTokenUsage>>
const FIRST_YEAR = 2026

export function LocalTokenActivity() {
  const { t, language } = useFrontendConfig()
  const today = shanghaiDate()
  const currentYear = Math.max(FIRST_YEAR, Number(today.slice(0, 4)))
  const [request, setRequest] = useState({ year: currentYear, revision: 0 })
  const [snapshot, setSnapshot] = useState<{ year: number; usage: Usage } | null>(null)
  const [busy, setBusy] = useState(true)
  const [error, setError] = useState(false)
  const requestRef = useRef(0)
  const year = snapshot?.year ?? request.year
  const usage = snapshot?.usage ?? null
  useEffect(() => {
    let cancelled = false
    const requestId = ++requestRef.current
    setBusy(true)
    setError(false)
    // Keep the displayed year and its data together until the local read succeeds.
    // Refreshing must not remove the calendar and collapse the profile's scroll height.
    void Promise.resolve()
      .then(() =>
        hostClient.agent.getLocalTokenUsage({
          from: `${request.year}-01-01`,
          to: `${request.year}-12-31`
        })
      )
      .then((result) => {
        if (!cancelled && requestId === requestRef.current)
          setSnapshot({ year: request.year, usage: result })
      })
      .catch(() => {
        if (!cancelled) setError(true)
      })
      .finally(() => {
        if (!cancelled) setBusy(false)
      })
    return () => {
      cancelled = true
    }
  }, [request])
  const readYear = (nextYear: number): void => {
    if (busy || nextYear < FIRST_YEAR || nextYear > currentYear) return
    setBusy(true)
    setRequest((previous) => ({ year: nextYear, revision: previous.revision + 1 }))
  }
  const points = useMemo(() => buildTokenYear(year, usage?.days ?? []), [year, usage])
  const peak = points.reduce((max, point) => (point.tokens > max ? point.tokens : max), BigInt(0))
  const yearTotal = points.reduce((total, point) => total + point.tokens, BigInt(0))
  const format = (tokens: string | bigint) =>
    new Intl.NumberFormat(language).format(
      typeof tokens === 'string' ? parseTokenCount(tokens) : tokens
    )
  const firstDate = usage?.startedAt ? shanghaiDate(usage.startedAt) : null
  const offset = new Date(Date.UTC(year, 0, 1)).getUTCDay()
  return (
    <section
      className="settings-list-section local-token-activity"
      aria-labelledby="local-token-title"
      data-setting-id="profile.localTokenUsage"
      aria-busy={busy}
    >
      <div className="local-token-activity__heading">
        <h2 id="local-token-title">{t('localUsage.title')}</h2>
        <button
          className="profile-settings-button local-token-activity__refresh"
          type="button"
          disabled={busy}
          onClick={() => readYear(year)}
        >
          <RefreshCw size={14} aria-hidden="true" data-spinning={busy || undefined} />
          {t('localUsage.refresh')}
        </button>
      </div>
      <div className="settings-list local-token-activity__summary">
        {(['today', 'total', 'peak'] as const).map((metric) => (
          <div key={metric}>
            <span>{t(`localUsage.${metric}`)}</span>
            <strong>
              {usage
                ? format(
                    metric === 'today'
                      ? usage.todayTokens
                      : metric === 'total'
                        ? usage.totalTokens
                        : usage.peakDailyTokens
                  )
                : '—'}
            </strong>
          </div>
        ))}
      </div>
      <div className="local-token-activity__controls">
        <div className="local-token-activity__year-navigation">
          <span>{t('localUsage.year')}</span>
          <div
            className="local-token-activity__year-picker"
            role="group"
            aria-label={t('localUsage.year')}
          >
            <button
              type="button"
              aria-label={t('localUsage.previousYear')}
              disabled={busy || year <= FIRST_YEAR}
              onClick={() => readYear(year - 1)}
            >
              <ChevronLeft size={16} aria-hidden="true" />
            </button>
            <span className="local-token-activity__year" aria-live="polite">
              {year}
            </span>
            <button
              type="button"
              aria-label={t('localUsage.nextYear')}
              disabled={busy || year >= currentYear}
              onClick={() => readYear(year + 1)}
            >
              <ChevronRight size={16} aria-hidden="true" />
            </button>
          </div>
        </div>
        <span>
          {t('localUsage.yearTotal')}: {usage ? format(yearTotal) : '—'}
        </span>
      </div>
      <div className="local-token-activity__feedback">
        <span className="local-token-activity__status" role="status">
          {busy ? t('localUsage.loading') : ''}
        </span>
        {error ? (
          <p className="profile-settings-error" role="alert">
            {t('localUsage.error')}
          </p>
        ) : null}
      </div>
      <div className="local-token-activity__calendar-scroll">
        <div className="local-token-activity__months" aria-hidden="true">
          {Array.from({ length: 12 }, (_, month) => (
            <span key={month}>
              {new Intl.DateTimeFormat(language, { month: 'short', timeZone: 'UTC' }).format(
                Date.UTC(year, month, 1)
              )}
            </span>
          ))}
        </div>
        <div
          className="local-token-activity__calendar"
          role="group"
          aria-label={t('localUsage.calendar')}
        >
          {Array.from({ length: offset }, (_, index) => (
            <span key={`blank-${index}`} aria-hidden="true" />
          ))}
          {points.map((point) => {
            const before = firstDate !== null && point.date < firstDate
            const future = point.date > today
            const title = `${point.date}: ${!usage ? t(error ? 'localUsage.error' : 'localUsage.loading') : before ? t('localUsage.beforeTracking') : `${format(point.tokens)} Token`}`
            return (
              <span
                key={point.date}
                className="local-token-activity__day"
                data-level={tokenIntensity(point.tokens, peak)}
                data-inactive={!usage || before || future || undefined}
                title={title}
                aria-label={title}
              />
            )
          })}
        </div>
      </div>
      <div className="local-token-activity__legend">
        <span>{t('localUsage.less')}</span>
        {[0, 1, 2, 3, 4].map((level) => (
          <span
            key={level}
            className="local-token-activity__day"
            data-level={level}
            aria-hidden="true"
          />
        ))}
        <span>{t('localUsage.more')}</span>
      </div>
    </section>
  )
}
