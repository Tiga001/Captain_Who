import { Download, FolderOpen, LoaderCircle, RefreshCw, Search, Trash2 } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  BrowserDownloadHistoryItem,
  BrowserDownloadHistoryListOutput
} from '@mycopilot/protocol'

import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { SettingsBreadcrumbs } from '../settings/components/SettingsBreadcrumbs'
import {
  clearBrowserDownloadHistory,
  listBrowserDownloadHistory,
  onBrowserDownloadHistoryChanged,
  revealBrowserDownload
} from './browserDownloadClient'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'

interface BrowserDownloadHistoryPageProps {
  onBack: () => void
  onNavigateSettingsRoot: () => void
}

export function BrowserDownloadHistoryPage({
  onBack,
  onNavigateSettingsRoot
}: BrowserDownloadHistoryPageProps) {
  const { language, t } = useFrontendConfig()
  const { showToast } = useToast()
  const [history, setHistory] = useState<BrowserDownloadHistoryListOutput | null>(null)
  const [query, setQuery] = useState('')
  const [refreshing, setRefreshing] = useState(false)
  const [mutating, setMutating] = useState(false)
  const historyRequestRevision = useRef(0)
  const initialLoadStarted = useRef(false)
  const lastScheduledQuery = useRef(query)

  const showFailure = useCallback(
    (error: unknown) => {
      showToast(
        error instanceof Error
          ? toSafeMcpDisplayText(error.message, 256)
          : t('mcp.browserDownloads.error'),
        { durationMs: 3200 }
      )
    },
    [showToast, t]
  )

  const loadHistory = useCallback(async (search: string) => {
    const revision = historyRequestRevision.current + 1
    historyRequestRevision.current = revision
    const output = await listBrowserDownloadHistory(search)
    if (historyRequestRevision.current === revision) setHistory(output)
    return output
  }, [])

  const refresh = useCallback(async () => {
    setRefreshing(true)
    try {
      await loadHistory(query)
    } catch (error) {
      showFailure(error)
    } finally {
      setRefreshing(false)
    }
  }, [loadHistory, query, showFailure])

  useEffect(() => {
    if (initialLoadStarted.current) return
    initialLoadStarted.current = true
    void loadHistory('').catch(showFailure)
  }, [loadHistory, showFailure])

  useEffect(() => {
    if (lastScheduledQuery.current === query) return
    lastScheduledQuery.current = query
    const timer = window.setTimeout(() => {
      void loadHistory(query).catch(showFailure)
    }, 180)
    return () => window.clearTimeout(timer)
  }, [loadHistory, query, showFailure])

  useEffect(
    () =>
      onBrowserDownloadHistoryChanged(() => {
        void loadHistory(query).catch(showFailure)
      }),
    [loadHistory, query, showFailure]
  )

  const groups = useMemo(
    () => groupDownloadsByDate(history?.downloads ?? [], language),
    [history?.downloads, language]
  )

  const clearHistory = async () => {
    if (mutating || !history?.downloads.length) return
    setMutating(true)
    try {
      await clearBrowserDownloadHistory()
      await loadHistory(query)
    } catch (error) {
      showFailure(error)
    } finally {
      setMutating(false)
    }
  }

  const reveal = async (item: BrowserDownloadHistoryItem) => {
    try {
      const output = await revealBrowserDownload(item.downloadId)
      if (output.status === 'missing') {
        showToast(t('mcp.browserDownloads.fileMissing'), { durationMs: 3200 })
        await loadHistory(query)
      }
    } catch (error) {
      showFailure(error)
    }
  }

  return (
    <article className="settings-list-page mcp-settings-page browser-download-history-page">
      <SettingsBreadcrumbs
        ariaLabel={t('settings.breadcrumb.label')}
        items={[
          {
            id: 'settings',
            label: t('settings.breadcrumb.root'),
            onSelect: onNavigateSettingsRoot
          },
          {
            id: 'browser',
            label: t('settings.nav.browser'),
            onSelect: onBack
          },
          { id: 'download-history', label: t('mcp.browserDownloads.history') }
        ]}
      />

      <h1>{t('mcp.browserDownloads.history')}</h1>

      <label className="browser-download-search">
        <Search aria-hidden="true" />
        <span className="sr-only">{t('mcp.browserDownloads.search')}</span>
        <input
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder={t('mcp.browserDownloads.search')}
          value={query}
        />
      </label>

      <div className="browser-download-history__heading">
        <h2>{t('mcp.browserDownloads.allHistory')}</h2>
        <div>
          <button
            aria-label={t('mcp.browserDownloads.refresh')}
            className="mcp-icon-button"
            disabled={refreshing}
            onClick={() => void refresh()}
            title={t('mcp.browserDownloads.refresh')}
            type="button"
          >
            <RefreshCw aria-hidden="true" className={refreshing ? 'mcp-spinner' : undefined} />
          </button>
          <button
            className="mcp-secondary-button"
            disabled={mutating || !history?.downloads.length}
            onClick={() => void clearHistory()}
            type="button"
          >
            <Trash2 aria-hidden="true" />
            {t('mcp.browserDownloads.clearHistory')}
          </button>
        </div>
      </div>

      {!history ? (
        <div className="mcp-page-state" role="status">
          <LoaderCircle aria-hidden="true" className="mcp-spinner" />
          {t('mcp.browserDownloads.loading')}
        </div>
      ) : groups.length ? (
        <div className="browser-download-history-groups">
          {groups.map((group) => (
            <section className="browser-download-history-group" key={group.key}>
              <h3>{group.label}</h3>
              <div className="browser-download-history-list">
                {group.items.map((item) => (
                  <article className="browser-download-history-row" key={item.downloadId}>
                    <Download aria-hidden="true" className="browser-download-history-row__icon" />
                    <div className="browser-download-history-row__copy">
                      <strong>{toSafeMcpDisplayText(item.displayName, 255)}</strong>
                      <p>{formatDownloadMeta(item, language, t)}</p>
                    </div>
                    <button
                      aria-label={t('mcp.browserDownloads.reveal')}
                      className="mcp-icon-button"
                      disabled={item.availability === 'missing'}
                      onClick={() => void reveal(item)}
                      title={t('mcp.browserDownloads.reveal')}
                      type="button"
                    >
                      <FolderOpen aria-hidden="true" />
                    </button>
                  </article>
                ))}
              </div>
            </section>
          ))}
        </div>
      ) : (
        <div className="browser-download-history-empty">
          <Download aria-hidden="true" />
          <strong>{t('mcp.browserDownloads.empty')}</strong>
          <p>{t('mcp.browserDownloads.emptyDescription')}</p>
        </div>
      )}
    </article>
  )
}

type Translate = ReturnType<typeof useFrontendConfig>['t']

function groupDownloadsByDate(
  downloads: readonly BrowserDownloadHistoryItem[],
  language: string
): Array<{ key: string; label: string; items: BrowserDownloadHistoryItem[] }> {
  const groups = new Map<string, { label: string; items: BrowserDownloadHistoryItem[] }>()
  const formatter = new Intl.DateTimeFormat(language, {
    year: 'numeric',
    month: 'long',
    day: 'numeric'
  })
  for (const item of downloads) {
    const date = new Date(item.createdAt)
    const key = `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
    const current = groups.get(key)
    if (current) current.items.push(item)
    else groups.set(key, { label: formatter.format(date), items: [item] })
  }
  return [...groups].map(([key, group]) => ({ key, ...group }))
}

function formatDownloadMeta(
  item: BrowserDownloadHistoryItem,
  language: string,
  t: Translate
): string {
  const parts = [
    formatBytes(item.sizeBytes, language),
    new Intl.DateTimeFormat(language, { timeStyle: 'short' }).format(item.createdAt),
    item.source === 'agent'
      ? t('mcp.browserDownloads.sourceAgent')
      : t('mcp.browserDownloads.sourceManual')
  ]
  const originHost = item.sourceOrigin ? safeOriginHost(item.sourceOrigin) : ''
  if (originHost) parts.push(originHost)
  if (item.availability === 'missing') parts.push(t('mcp.browserDownloads.missing'))
  if (item.availability === 'modified') parts.push(t('mcp.browserDownloads.modified'))
  return parts.join(' · ')
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

function safeOriginHost(origin: string): string {
  try {
    return new URL(origin).host
  } catch {
    return ''
  }
}
