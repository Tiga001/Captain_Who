import { Cookie, Download, Globe2, Images, X } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type {
  BrowserDataCategory,
  BrowserDataSummaryOutput,
  BrowserDataTimeRange
} from '@mycopilot/protocol'

import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { clearBrowserData, getBrowserDataSummary } from './browserDataClient'
import './ClearBrowsingDataDialog.css'

interface ClearBrowsingDataDialogProps {
  onClose: () => void
  onCleared?: () => void
}

const TIME_RANGES: BrowserDataTimeRange[] = [
  'lastHour',
  'last24Hours',
  'last7Days',
  'last4Weeks',
  'allTime'
]

const ALL_CATEGORIES: BrowserDataCategory[] = [
  'history',
  'cookiesAndSiteData',
  'cache',
  'downloadHistory'
]

export function ClearBrowsingDataDialog({ onClose, onCleared }: ClearBrowsingDataDialogProps) {
  const { language, t } = useFrontendConfig()
  const titleId = useId()
  const cardRef = useRef<HTMLElement>(null)
  const closeButtonRef = useRef<HTMLButtonElement>(null)
  const [timeRange, setTimeRange] = useState<BrowserDataTimeRange>('allTime')
  const [categories, setCategories] = useState<Set<BrowserDataCategory>>(
    () => new Set(ALL_CATEGORIES)
  )
  const [summary, setSummary] = useState<BrowserDataSummaryOutput | null>(null)
  const [loading, setLoading] = useState(true)
  const [clearing, setClearing] = useState(false)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const frame = window.requestAnimationFrame(() => closeButtonRef.current?.focus())
    return () => {
      window.cancelAnimationFrame(frame)
      previous?.isConnected && previous.focus({ preventScroll: true })
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setFailed(false)
    void getBrowserDataSummary(timeRange)
      .then((output) => {
        if (!cancelled) setSummary(output)
      })
      .catch(() => {
        if (!cancelled) setFailed(true)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [timeRange])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.preventDefault()
        if (!clearing) onClose()
        return
      }
      if (event.key !== 'Tab') return
      const controls = Array.from(
        cardRef.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), input:not(:disabled)'
        ) ?? []
      )
      const first = controls[0]
      const last = controls.at(-1)
      if (!first || !last) return
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [clearing, onClose])

  const rows = useMemo(
    () => [
      {
        category: 'history' as const,
        icon: Globe2,
        label: t('mcp.browserData.history'),
        summary: formatTranslation(t, 'mcp.browserData.historySummary', {
          count: summary?.historyCount ?? 0,
          sites: summary?.historySiteCount ?? 0
        })
      },
      {
        category: 'cookiesAndSiteData' as const,
        icon: Cookie,
        label: t('mcp.browserData.cookies'),
        summary: formatTranslation(t, 'mcp.browserData.cookiesSummary', {
          count: summary?.cookieSiteCount ?? 0
        })
      },
      {
        category: 'cache' as const,
        icon: Images,
        label: t('mcp.browserData.cache'),
        summary: formatBytes(summary?.cacheBytes ?? 0, language)
      },
      {
        category: 'downloadHistory' as const,
        icon: Download,
        label: t('mcp.browserData.downloadHistory'),
        summary: formatTranslation(t, 'mcp.browserData.downloadSummary', {
          count: summary?.downloadCount ?? 0
        })
      }
    ],
    [language, summary, t]
  )

  const selectRange = (nextRange: BrowserDataTimeRange): void => {
    setTimeRange(nextRange)
    if (nextRange !== 'allTime') {
      setCategories((current) => {
        const next = new Set(current)
        next.delete('cookiesAndSiteData')
        next.delete('cache')
        return next
      })
    }
  }

  const toggleCategory = (category: BrowserDataCategory): void => {
    setCategories((current) => {
      const next = new Set(current)
      if (next.has(category)) next.delete(category)
      else next.add(category)
      return next
    })
  }

  const confirmClear = async (): Promise<void> => {
    if (clearing || categories.size === 0) return
    setClearing(true)
    setFailed(false)
    try {
      await clearBrowserData(timeRange, [...categories])
      onCleared?.()
      onClose()
    } catch {
      setFailed(true)
    } finally {
      setClearing(false)
    }
  }

  return createPortal(
    <div
      className="browser-data-dialog__backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (!clearing && event.currentTarget === event.target) onClose()
      }}
    >
      <section
        aria-busy={clearing || loading || undefined}
        aria-labelledby={titleId}
        aria-modal="true"
        className="browser-data-dialog"
        ref={cardRef}
        role="dialog"
      >
        <header>
          <h2 id={titleId}>{t('browser.clearBrowsingData')}</h2>
          <button
            aria-label={t('mcp.actions.cancel')}
            className="browser-data-dialog__close"
            disabled={clearing}
            onClick={onClose}
            ref={closeButtonRef}
            type="button"
          >
            <X aria-hidden="true" />
          </button>
        </header>

        <div className="browser-data-dialog__ranges" role="group">
          {TIME_RANGES.map((range) => (
            <button
              aria-pressed={timeRange === range}
              data-active={timeRange === range || undefined}
              disabled={clearing}
              key={range}
              onClick={() => selectRange(range)}
              type="button"
            >
              {t(`mcp.browserData.range.${range}`)}
            </button>
          ))}
        </div>

        <div className="browser-data-dialog__categories">
          {rows.map((row) => {
            const allTimeOnly = row.category === 'cookiesAndSiteData' || row.category === 'cache'
            const disabled = clearing || (allTimeOnly && timeRange !== 'allTime')
            const Icon = row.icon
            return (
              <label aria-disabled={disabled || undefined} key={row.category}>
                <Icon aria-hidden="true" />
                <span>
                  <strong>{row.label}</strong>
                  <small>
                    {allTimeOnly && timeRange !== 'allTime'
                      ? t('mcp.browserData.allTimeOnly')
                      : row.summary}
                  </small>
                </span>
                <input
                  checked={categories.has(row.category)}
                  disabled={disabled}
                  onChange={() => toggleCategory(row.category)}
                  type="checkbox"
                />
              </label>
            )
          })}
        </div>

        {failed && <p className="browser-data-dialog__error">{t('mcp.browserData.failed')}</p>}

        <footer>
          <button disabled={clearing} onClick={onClose} type="button">
            {t('mcp.actions.cancel')}
          </button>
          <button
            className="browser-data-dialog__confirm"
            disabled={clearing || categories.size === 0}
            onClick={() => void confirmClear()}
            type="button"
          >
            {t('mcp.browserData.clear')}
          </button>
        </footer>
      </section>
    </div>,
    document.body
  )
}

function formatBytes(bytes: number, language: string): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = units[0]
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024
    unit = units[index]
  }
  return `${new Intl.NumberFormat(language, { maximumFractionDigits: value < 10 ? 1 : 0 }).format(value)} ${unit}`
}
