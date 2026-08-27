import {
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Globe2,
  LoaderCircle,
  MoreHorizontal,
  Search,
  Trash2
} from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type { BrowserHistoryEntry, BrowserHistoryListOutput } from '@mycopilot/protocol'

import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { hostClient } from '../../host/hostClient'
import { ClearBrowsingDataDialog } from '../browser/ClearBrowsingDataDialog'
import { SettingsBreadcrumbs } from '../settings/components/SettingsBreadcrumbs'
import {
  deleteBrowserHistory,
  listBrowserHistory,
  onBrowserHistoryChanged,
  openBrowserHistoryEntry
} from '../browser/browserDataClient'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'

interface BrowserHistoryPageProps {
  onBack: () => void
  onCloseSettings?: () => void
  onNavigateMcp: () => void
  onNavigateSettingsRoot: () => void
}

interface HistoryMenuState {
  entry: BrowserHistoryEntry
  left: number
  top: number
}

export function BrowserHistoryPage({
  onBack,
  onCloseSettings,
  onNavigateMcp,
  onNavigateSettingsRoot
}: BrowserHistoryPageProps) {
  const { language, t } = useFrontendConfig()
  const { showToast } = useToast()
  const [history, setHistory] = useState<BrowserHistoryListOutput | null>(null)
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<Set<string>>(() => new Set())
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const [menu, setMenu] = useState<HistoryMenuState | null>(null)
  const [clearDialogOpen, setClearDialogOpen] = useState(false)
  const [mutating, setMutating] = useState(false)
  const requestRevision = useRef(0)

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
  const showFailureRef = useRef(showFailure)

  useEffect(() => {
    showFailureRef.current = showFailure
  }, [showFailure])

  const loadHistory = useCallback(async (search: string) => {
    const revision = requestRevision.current + 1
    requestRevision.current = revision
    const output = await listBrowserHistory(search)
    if (requestRevision.current === revision) {
      setHistory(output)
      const identities = new Set(output.entries.map((entry) => entry.historyId))
      setSelected((current) => new Set([...current].filter((id) => identities.has(id))))
    }
  }, [])

  useEffect(() => {
    const timer = window.setTimeout(
      () => {
        void loadHistory(query).catch((error) => showFailureRef.current(error))
      },
      query ? 180 : 0
    )
    return () => window.clearTimeout(timer)
  }, [loadHistory, query])

  useEffect(
    () =>
      onBrowserHistoryChanged(() => {
        void loadHistory(query).catch((error) => showFailureRef.current(error))
      }),
    [loadHistory, query]
  )

  useEffect(() => {
    if (!menu) return
    const dismiss = (event: PointerEvent): void => {
      if (event.target instanceof Element && event.target.closest('.browser-history-menu')) return
      setMenu(null)
    }
    const close = (): void => setMenu(null)
    window.addEventListener('pointerdown', dismiss, true)
    window.addEventListener('resize', close)
    window.addEventListener('scroll', close, true)
    return () => {
      window.removeEventListener('pointerdown', dismiss, true)
      window.removeEventListener('resize', close)
      window.removeEventListener('scroll', close, true)
    }
  }, [menu])

  const groups = useMemo(
    () => groupHistoryByDate(history?.entries ?? [], language),
    [history?.entries, language]
  )

  const toggleSelectedEntry = (historyId: string): void => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(historyId)) next.delete(historyId)
      else next.add(historyId)
      return next
    })
  }

  const removeEntries = async (historyIds: string[]): Promise<void> => {
    if (mutating || historyIds.length === 0) return
    setMutating(true)
    try {
      await deleteBrowserHistory(historyIds)
      setSelected((current) => {
        const next = new Set(current)
        historyIds.forEach((id) => next.delete(id))
        return next
      })
      await loadHistory(query)
    } catch (error) {
      showFailure(error)
    } finally {
      setMutating(false)
    }
  }

  const openEntry = async (entry: BrowserHistoryEntry): Promise<void> => {
    setMenu(null)
    try {
      await openBrowserHistoryEntry(entry.url)
      onCloseSettings?.()
    } catch (error) {
      showFailure(error)
    }
  }

  return (
    <article className="settings-list-page mcp-settings-page browser-history-page">
      <SettingsBreadcrumbs
        ariaLabel={t('settings.breadcrumb.label')}
        items={[
          {
            id: 'settings',
            label: t('settings.breadcrumb.root'),
            onSelect: onNavigateSettingsRoot
          },
          { id: 'mcp', label: t('settings.nav.mcp'), onSelect: onNavigateMcp },
          {
            id: 'browser-automation',
            label: t('mcp.builtin.browserAutomation.name'),
            onSelect: onBack
          },
          { id: 'browser-history', label: t('browser.history') }
        ]}
      />

      <h1>{t('browser.history')}</h1>

      <label className="browser-download-search">
        <Search aria-hidden="true" />
        <span className="sr-only">{t('mcp.browserData.searchHistory')}</span>
        <input
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder={t('mcp.browserData.searchHistory')}
          value={query}
        />
      </label>

      <div className="browser-download-history__heading">
        <h2>{t('mcp.browserDownloads.allHistory')}</h2>
        {selected.size > 0 ? (
          <button
            className="mcp-secondary-button"
            disabled={mutating}
            onClick={() => void removeEntries([...selected])}
            type="button"
          >
            {t('mcp.browserData.removeSelected')}
          </button>
        ) : (
          <button
            className="mcp-secondary-button"
            onClick={() => setClearDialogOpen(true)}
            type="button"
          >
            {t('browser.clearBrowsingData')}
          </button>
        )}
      </div>

      {!history ? (
        <div className="mcp-page-state" role="status">
          <LoaderCircle aria-hidden="true" className="mcp-spinner" />
          {t('mcp.browserDownloads.loading')}
        </div>
      ) : groups.length > 0 ? (
        <div className="browser-history-groups">
          {groups.map((group) => {
            const isCollapsed = collapsed.has(group.key)
            return (
              <section className="browser-history-group" key={group.key}>
                <button
                  aria-expanded={!isCollapsed}
                  className="browser-history-group__header"
                  onClick={() =>
                    setCollapsed((current) => {
                      const next = new Set(current)
                      if (next.has(group.key)) next.delete(group.key)
                      else next.add(group.key)
                      return next
                    })
                  }
                  type="button"
                >
                  <strong>{group.label}</strong>
                  {isCollapsed ? (
                    <ChevronDown aria-hidden="true" />
                  ) : (
                    <ChevronUp aria-hidden="true" />
                  )}
                </button>
                {!isCollapsed && (
                  <div className="browser-history-list">
                    {group.entries.map((entry) => (
                      <div
                        className="browser-history-row"
                        data-selected={selected.has(entry.historyId) || undefined}
                        key={entry.historyId}
                        onClick={(event) => {
                          if (
                            event.target instanceof Element &&
                            event.target.closest('button, input')
                          ) {
                            return
                          }
                          toggleSelectedEntry(entry.historyId)
                        }}
                      >
                        <input
                          aria-label={entry.title}
                          checked={selected.has(entry.historyId)}
                          onChange={() => toggleSelectedEntry(entry.historyId)}
                          type="checkbox"
                        />
                        <HistoryFavicon entry={entry} />
                        <div className="browser-history-row__link">
                          <strong>{toSafeMcpDisplayText(entry.title, 256)}</strong>
                          <span>{entry.hostname}</span>
                        </div>
                        <time>{formatTime(entry.visitedAt, language)}</time>
                        <button
                          aria-label={t('browser.downloadCenter.moreActions')}
                          className="mcp-icon-button"
                          onClick={(event) => {
                            const bounds = event.currentTarget.getBoundingClientRect()
                            setMenu({
                              entry,
                              left: Math.max(
                                8,
                                Math.min(window.innerWidth - 228, bounds.right - 220)
                              ),
                              top: Math.min(window.innerHeight - 108, bounds.bottom + 6)
                            })
                          }}
                          type="button"
                        >
                          <MoreHorizontal aria-hidden="true" />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </section>
            )
          })}
        </div>
      ) : (
        <div className="browser-download-history-empty">
          <Globe2 aria-hidden="true" />
          <strong>{t('mcp.browserData.emptyHistory')}</strong>
          <p>{t('mcp.browserData.emptyHistoryDescription')}</p>
        </div>
      )}

      {menu
        ? createPortal(
            <div className="browser-history-menu" style={{ left: menu.left, top: menu.top }}>
              <button onClick={() => void openEntry(menu.entry)} type="button">
                <ExternalLink aria-hidden="true" />
                {t('browser.open')}
              </button>
              <button
                onClick={() => {
                  const id = menu.entry.historyId
                  setMenu(null)
                  void removeEntries([id])
                }}
                type="button"
              >
                <Trash2 aria-hidden="true" />
                {t('mcp.browserData.removeFromHistory')}
              </button>
            </div>,
            document.body
          )
        : null}

      {clearDialogOpen && (
        <ClearBrowsingDataDialog
          onClose={() => setClearDialogOpen(false)}
          onCleared={() => void loadHistory(query).catch(showFailure)}
        />
      )}
    </article>
  )
}

function HistoryFavicon({ entry }: { entry: BrowserHistoryEntry }) {
  const [favicon, setFavicon] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let cancelled = false
    setFavicon(null)
    setFailed(false)
    void hostClient.resources
      .resolveFavicon({ pageUrl: entry.url, faviconUrl: entry.faviconUrl })
      .then((result) => {
        if (!cancelled) setFavicon(result.url)
      })
      .catch(() => {
        if (!cancelled) setFavicon(null)
      })
    return () => {
      cancelled = true
    }
  }, [entry.faviconUrl, entry.url])

  return favicon && !failed ? (
    <img alt="" onError={() => setFailed(true)} src={favicon} />
  ) : (
    <Globe2 aria-hidden="true" />
  )
}

function groupHistoryByDate(
  entries: readonly BrowserHistoryEntry[],
  language: string
): Array<{ key: string; label: string; entries: BrowserHistoryEntry[] }> {
  const groups = new Map<string, { label: string; entries: BrowserHistoryEntry[] }>()
  const formatter = new Intl.DateTimeFormat(language, {
    year: 'numeric',
    month: 'long',
    day: 'numeric'
  })
  for (const entry of entries) {
    const date = new Date(entry.visitedAt)
    const key = `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`
    const current = groups.get(key)
    if (current) current.entries.push(entry)
    else groups.set(key, { label: formatter.format(date), entries: [entry] })
  }
  return [...groups].map(([key, group]) => ({ key, ...group }))
}

function formatTime(timestamp: number, language: string): string {
  return new Intl.DateTimeFormat(language, { timeStyle: 'short' }).format(timestamp)
}
