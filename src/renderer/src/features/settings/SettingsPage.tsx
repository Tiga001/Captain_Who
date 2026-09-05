import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { ArrowLeft, Search, X } from 'lucide-react'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { isMacOS } from '../../lib/platform'
import type { AppProject } from '../../config/projectConfig'
import type { ChatConversation } from '../chat/chatTypes'
import type { UiPreferencesSnapshot } from '../storage/storageClient'
import { getTranslucentSidebarOpacityPercent } from '../storage/storageClient'
import { McpSettingsPage } from '../mcp/McpSettingsPage'
import type { BrowserAutomationView } from '../mcp/BrowserAutomationSettingsPage'
import { BrowserAutomationSettingsPage } from '../mcp/BrowserAutomationSettingsPage'
import { AppearanceSettingsPage } from './pages/AppearanceSettingsPage'
import { ArchivedConversationsSettingsPage } from './pages/ArchivedConversationsSettingsPage'
import { ConfigurationSettingsPage } from './pages/ConfigurationSettingsPage'
import { EnvironmentSettingsPage } from './pages/EnvironmentSettingsPage'
import { GeneralSettingsPage } from './pages/GeneralSettingsPage'
import { PersonalizationSettingsPage } from './pages/PersonalizationSettingsPage'
import { ProfileSettingsPage } from './pages/ProfileSettingsPage'
import { SkillsSettingsPage } from './pages/SkillsSettingsPage'
import { UsageBillingSettingsPage } from './pages/UsageBillingSettingsPage'
import { AgentTemplatesSettingsPage } from './pages/AgentTemplatesSettingsPage'
import { SETTINGS_GROUPS, type SettingsPageId } from './settingsRegistry'
import {
  buildSettingsSearchIndex,
  searchSettings,
  type SettingsSearchResult
} from './settingsSearch'
import {
  SettingsSearchNavigationProvider,
  type SettingsNavigationTarget
} from './settingsSearchNavigation'
export type { SettingsPageId } from './settingsRegistry'
import './SettingsPage.css'

const SUPPORTS_NATIVE_FONT_SMOOTHING = isMacOS()

interface SettingsPageProps {
  conversations: ChatConversation[]
  onBack: () => void
  onDeleteArchivedConversations: (conversationIds: string[]) => void
  onDeleteConversation: (conversationId: string) => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  onUnarchiveConversation: (conversationId: string) => void
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  projects: AppProject[]
  initialProjectId?: string | null
  initialPage?: SettingsPageId
  initialBrowserView?: BrowserAutomationView
  uiPreferences: UiPreferencesSnapshot
}

function SettingsContent({
  activePage,
  conversations,
  onDeleteArchivedConversations,
  onDeleteConversation,
  onMcpDirtyChange,
  onSelectSettingsPage,
  onRemoveProject,
  onUnarchiveConversation,
  onUiPreferencesChange,
  initialProjectId,
  initialBrowserView,
  browserPageRevision,
  onCloseSettings,
  projects,
  uiPreferences
}: {
  activePage: SettingsPageId
  conversations: ChatConversation[]
  onDeleteArchivedConversations: (conversationIds: string[]) => void
  onDeleteConversation: (conversationId: string) => void
  onMcpDirtyChange: (dirty: boolean) => void
  onSelectSettingsPage: (page: SettingsPageId) => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  onUnarchiveConversation: (conversationId: string) => void
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  initialProjectId?: string | null
  initialBrowserView?: BrowserAutomationView
  browserPageRevision: number
  onCloseSettings: () => void
  projects: AppProject[]
  uiPreferences: UiPreferencesSnapshot
}) {
  if (activePage === 'appearance') {
    return (
      <AppearanceSettingsPage
        uiPreferences={uiPreferences}
        onUiPreferencesChange={onUiPreferencesChange}
      />
    )
  }

  if (activePage === 'profile') {
    return (
      <ProfileSettingsPage
        uiPreferences={uiPreferences}
        onUiPreferencesChange={onUiPreferencesChange}
      />
    )
  }

  if (activePage === 'configuration') {
    return (
      <ConfigurationSettingsPage onNavigateSettingsRoot={() => onSelectSettingsPage('general')} />
    )
  }

  if (activePage === 'personalization') {
    return <PersonalizationSettingsPage />
  }

  if (activePage === 'usageBilling') {
    return (
      <UsageBillingSettingsPage
        uiPreferences={uiPreferences}
        onUiPreferencesChange={onUiPreferencesChange}
      />
    )
  }

  if (activePage === 'skills') {
    return <SkillsSettingsPage />
  }

  if (activePage === 'agentTemplates') {
    return (
      <AgentTemplatesSettingsPage
        initialProjectId={initialProjectId}
        onNavigateSettingsRoot={() => onSelectSettingsPage('general')}
        projects={projects}
      />
    )
  }

  if (activePage === 'mcp') {
    return (
      <McpSettingsPage
        onDirtyChange={onMcpDirtyChange}
        onNavigateSettingsRoot={() => onSelectSettingsPage('general')}
      />
    )
  }

  if (activePage === 'browser') {
    return (
      <BrowserAutomationSettingsPage
        key={browserPageRevision}
        initialView={initialBrowserView}
        onCloseSettings={onCloseSettings}
        onNavigateSettingsRoot={() => onSelectSettingsPage('general')}
      />
    )
  }

  if (activePage === 'environment') {
    return <EnvironmentSettingsPage onRemoveProject={onRemoveProject} />
  }

  if (activePage === 'archivedConversations') {
    return (
      <ArchivedConversationsSettingsPage
        conversations={conversations}
        projects={projects}
        onDeleteArchivedConversations={onDeleteArchivedConversations}
        onDeleteConversation={onDeleteConversation}
        onUnarchiveConversation={onUnarchiveConversation}
      />
    )
  }

  return (
    <GeneralSettingsPage
      uiPreferences={uiPreferences}
      onUiPreferencesChange={onUiPreferencesChange}
    />
  )
}

interface SettingsNavigationProps {
  activePage: SettingsPageId
  onBack: () => void
  onSelectPage: (page: SettingsPageId) => void
  onSelectSetting: (result: SettingsSearchResult) => void
  onClearSearch: () => void
  target: SettingsNavigationTarget | null
  locationStatus: 'waiting' | 'context' | 'located'
}

function HighlightedSettingText({ text, query }: { text: string; query: string }) {
  const match = text.toLocaleLowerCase().indexOf(query.trim().toLocaleLowerCase())
  if (match < 0 || !query.trim()) return <>{text}</>
  const end = match + query.trim().length
  return (
    <>
      {text.slice(0, match)}
      <strong>{text.slice(match, end)}</strong>
      {text.slice(end)}
    </>
  )
}

function SettingsNavigation({
  activePage,
  onBack,
  onSelectPage,
  onSelectSetting,
  onClearSearch,
  target,
  locationStatus
}: SettingsNavigationProps) {
  const { t } = useFrontendConfig()
  const [searchQuery, setSearchQuery] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const resultsRef = useRef<HTMLElement>(null)
  const isSearching = searchQuery.trim().length > 0
  const pages = SETTINGS_GROUPS.flatMap((group) => group.items)
  const index = useMemo(
    () =>
      buildSettingsSearchIndex(
        SETTINGS_GROUPS.flatMap((group) => group.items),
        t
      ),
    [t]
  )
  const results = useMemo(() => searchSettings(index, searchQuery), [index, searchQuery])
  const resultPages = [...new Set(results.map((result) => result.page))]
    .map((id) => pages.find((page) => page.id === id))
    .filter((page) => page !== undefined)
  const clearSearch = () => {
    setSearchQuery('')
    onClearSearch()
    inputRef.current?.focus()
  }
  return (
    <aside className="settings-nav" aria-label={t('settings.navigation')}>
      <button className="settings-nav__back" type="button" onClick={onBack}>
        <ArrowLeft aria-hidden="true" />
        <span>{t('settings.backToApp')}</span>
      </button>
      <label className="settings-nav__search">
        <Search aria-hidden="true" />
        <input
          ref={inputRef}
          type="search"
          placeholder={t('settings.searchPlaceholder')}
          aria-label={t('settings.search')}
          value={searchQuery}
          onChange={(event) => {
            const query = event.currentTarget.value
            setSearchQuery(query)
            if (!query.trim()) onClearSearch()
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing || event.keyCode === 229) return
            if (event.key === 'Escape' && searchQuery) {
              event.preventDefault()
              event.stopPropagation()
              clearSearch()
            }
            if (event.key === 'ArrowDown' && results.length) {
              event.preventDefault()
              resultsRef.current?.querySelector<HTMLButtonElement>('[data-setting-result]')?.focus()
            }
            if (event.key === 'Enter' && results[0]) {
              event.preventDefault()
              onSelectSetting(results[0])
            }
          }}
        />
        {searchQuery && (
          <button
            className="settings-nav__clear"
            type="button"
            aria-label={t('settings.search.clear')}
            onClick={clearSearch}
          >
            <X aria-hidden="true" />
          </button>
        )}
      </label>
      {isSearching ? (
        <nav
          ref={resultsRef}
          className="settings-nav__groups settings-nav__results"
          aria-label={t('settings.search.results')}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return
            if (event.key === 'Escape') {
              event.preventDefault()
              event.stopPropagation()
              clearSearch()
              return
            }
            if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
            const buttons = [
              ...(resultsRef.current?.querySelectorAll<HTMLButtonElement>(
                '[data-setting-result]'
              ) ?? [])
            ]
            const current = buttons.indexOf(document.activeElement as HTMLButtonElement)
            if (current < 0) return
            event.preventDefault()
            const next = current + (event.key === 'ArrowDown' ? 1 : -1)
            if (next < 0) inputRef.current?.focus()
            else buttons[Math.min(next, buttons.length - 1)]?.focus()
          }}
        >
          {results.length === 0 && (
            <div className="settings-nav__empty" role="status">
              <p>{t('settings.search.empty')}</p>
              <small>{t('settings.search.emptyHint')}</small>
            </div>
          )}
          {resultPages.map((item) => {
            const matches = results.filter((result) => result.page === item.id)
            if (!matches.length) return null
            const Icon = item.icon
            return (
              <section
                className="settings-nav__result-group"
                key={item.id}
                aria-label={t(item.labelKey)}
              >
                <button
                  className="settings-nav__item settings-nav__result-page"
                  type="button"
                  onClick={() => onSelectPage(item.id)}
                >
                  <Icon aria-hidden="true" />
                  <span>{t(item.labelKey)}</span>
                </button>
                {matches.map((result) => {
                  const selected = target?.page === result.page && target.id === result.id
                  const path = result.path.slice(1).join(' › ')
                  return (
                    <button
                      className="settings-nav__result"
                      data-setting-result
                      data-active={selected || undefined}
                      aria-current={selected ? 'location' : undefined}
                      type="button"
                      key={result.id}
                      onClick={() => onSelectSetting(result)}
                      title={[...result.path, result.label, result.description]
                        .filter(Boolean)
                        .join(' › ')}
                    >
                      <span className="settings-nav__result-title">
                        <HighlightedSettingText text={result.label} query={searchQuery} />
                      </span>
                      {path && <span className="settings-nav__result-path">{path}</span>}
                      {selected && locationStatus !== 'located' && (
                        <span className="settings-nav__result-status" role="status">
                          {t(
                            locationStatus === 'context'
                              ? 'settings.search.context'
                              : 'settings.search.waiting'
                          )}
                        </span>
                      )}
                    </button>
                  )
                })}
              </section>
            )
          })}
        </nav>
      ) : (
        <nav className="settings-nav__groups">
          {SETTINGS_GROUPS.map((group) => (
            <section
              className="settings-nav__group"
              key={group.titleKey}
              aria-labelledby={`settings-${group.titleKey}`}
            >
              <h2 id={`settings-${group.titleKey}`}>{t(group.titleKey)}</h2>
              <div className="settings-nav__items">
                {group.items.map((item) => {
                  const Icon = item.icon
                  return (
                    <button
                      className="settings-nav__item"
                      data-active={activePage === item.id || undefined}
                      type="button"
                      key={item.id}
                      onClick={() => onSelectPage(item.id)}
                    >
                      <Icon aria-hidden="true" />
                      <span>{t(item.labelKey)}</span>
                    </button>
                  )
                })}
              </div>
            </section>
          ))}
        </nav>
      )}
    </aside>
  )
}

export function SettingsPage({
  conversations,
  onBack,
  onDeleteArchivedConversations,
  onDeleteConversation,
  onRemoveProject,
  onUnarchiveConversation,
  onUiPreferencesChange,
  projects,
  initialProjectId,
  initialBrowserView,
  initialPage = 'general',
  uiPreferences
}: SettingsPageProps) {
  const { t } = useFrontendConfig()
  const [activePage, setActivePage] = useState<SettingsPageId>(initialPage)
  const [browserEntryView, setBrowserEntryView] = useState<BrowserAutomationView | undefined>(
    initialBrowserView
  )
  const [browserPageRevision, setBrowserPageRevision] = useState(0)
  const [mcpDirty, setMcpDirty] = useState(false)
  const [searchTarget, setSearchTarget] = useState<SettingsNavigationTarget | null>(null)
  const [locationStatus, setLocationStatus] = useState<'waiting' | 'context' | 'located'>('located')
  const searchRevision = useRef(0)
  const contentRef = useRef<HTMLElement>(null)
  const [pendingNavigation, setPendingNavigation] = useState<
    | { type: 'back' }
    | { type: 'page'; page: SettingsPageId }
    | { type: 'setting'; target: SettingsNavigationTarget }
    | null
  >(null)
  const pageRef = useRef<HTMLDivElement>(null)
  const handleMcpDirtyChange = useCallback((dirty: boolean) => {
    setMcpDirty(dirty)
  }, [])

  const requestPage = (page: SettingsPageId) => {
    if (activePage === 'mcp' && mcpDirty && page !== activePage) {
      setPendingNavigation({ type: 'page', page })
      return
    }
    setSearchTarget(null)
    if (page === 'browser') {
      setBrowserEntryView(undefined)
      if (activePage === 'browser') setBrowserPageRevision((current) => current + 1)
    } else if (activePage === 'browser') {
      setBrowserEntryView(undefined)
    }
    if (page === activePage) return
    setActivePage(page)
  }

  const applySearchTarget = (target: SettingsNavigationTarget) => {
    setLocationStatus('waiting')
    setSearchTarget(target)
    setActivePage(target.page as SettingsPageId)
  }

  const requestSetting = (result: SettingsSearchResult) => {
    const target: SettingsNavigationTarget = {
      page: result.page,
      id: result.id,
      view: result.view,
      prerequisiteId: result.prerequisiteId,
      ancestorIds: result.ancestorIds,
      revision: ++searchRevision.current
    }
    if (activePage === 'mcp' && mcpDirty && (target.page !== 'mcp' || target.view !== 'editor')) {
      setPendingNavigation({ type: 'setting', target })
      return
    }
    applySearchTarget(target)
  }

  const requestBack = () => {
    if (activePage === 'mcp' && mcpDirty) {
      setPendingNavigation({ type: 'back' })
      return
    }
    onBack()
  }

  const confirmPendingNavigation = () => {
    const navigation = pendingNavigation
    setPendingNavigation(null)
    setMcpDirty(false)
    if (!navigation) return
    if (navigation.type === 'back') {
      onBack()
      return
    }
    if (navigation.type === 'setting') {
      applySearchTarget(navigation.target)
      return
    }
    setSearchTarget(null)
    setActivePage(navigation.page)
  }

  useEffect(() => {
    const content = contentRef.current
    if (!content || !searchTarget || searchTarget.page !== activePage) return
    let frame = 0
    let fallbackLocated = false
    const findVisible = (id: string) =>
      [...document.querySelectorAll<HTMLElement>('[data-setting-id]')].find((element) => {
        if (element.dataset.settingId !== id || element.closest('[aria-hidden="true"], [inert]'))
          return false
        const rect = element.getBoundingClientRect()
        const style = getComputedStyle(element)
        return rect.height > 0 && rect.width > 0 && style.visibility !== 'hidden'
      })
    const scrollTo = (element: HTMLElement) => {
      if (!content.contains(element)) {
        element.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' })
        return
      }
      content.scrollTo({
        top: Math.max(
          0,
          content.scrollTop +
            element.getBoundingClientRect().top -
            content.getBoundingClientRect().top -
            24
        ),
        behavior: 'instant'
      })
    }
    const locate = () => {
      const target = findVisible(searchTarget.id)
      if (target) {
        scrollTo(target)
        setLocationStatus('located')
        observer.disconnect()
        return
      }
      const prerequisiteIds = [
        searchTarget.prerequisiteId,
        ...(searchTarget.ancestorIds ?? [])
      ].filter((id): id is string => Boolean(id))
      if (prerequisiteIds.length) {
        const prerequisite = prerequisiteIds.map(findVisible).find(Boolean)
        if (prerequisite && !fallbackLocated) {
          scrollTo(prerequisite)
          fallbackLocated = true
        }
        setLocationStatus('context')
      }
    }
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(locate)
    }
    const observer = new MutationObserver(schedule)
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['aria-hidden', 'data-open', 'style']
    })
    schedule()
    return () => {
      cancelAnimationFrame(frame)
      observer.disconnect()
    }
  }, [activePage, searchTarget])

  useEffect(() => {
    setActivePage(initialPage)
  }, [initialPage])

  useEffect(() => {
    setBrowserEntryView(initialBrowserView)
  }, [initialBrowserView])

  useEffect(() => {
    pageRef.current?.focus({ preventScroll: true })
  }, [])

  return (
    <div
      className="settings-page"
      ref={pageRef}
      tabIndex={-1}
      data-native-font-smoothing={
        SUPPORTS_NATIVE_FONT_SMOOTHING && uiPreferences.nativeFontSmoothing ? 'true' : undefined
      }
      data-translucent-sidebar={uiPreferences.translucentSidebar || undefined}
      style={
        {
          '--mc-sidebar-translucent-opacity': getTranslucentSidebarOpacityPercent(
            uiPreferences.translucentSidebarTransparency
          )
        } as CSSProperties
      }
      onContextMenu={(event) => {
        event.preventDefault()
      }}
    >
      <div className="settings-page__drag-region" data-drag-region />
      <SettingsNavigation
        activePage={activePage}
        onBack={requestBack}
        onSelectPage={requestPage}
        onSelectSetting={requestSetting}
        onClearSearch={() => setSearchTarget(null)}
        target={searchTarget}
        locationStatus={locationStatus}
      />

      <main className="settings-content" ref={contentRef} aria-label={t('settings.content')}>
        <div className="settings-content__inner">
          <SettingsSearchNavigationProvider target={searchTarget}>
            <SettingsContent
              activePage={activePage}
              conversations={conversations}
              projects={projects}
              onDeleteArchivedConversations={onDeleteArchivedConversations}
              onDeleteConversation={onDeleteConversation}
              onMcpDirtyChange={handleMcpDirtyChange}
              onSelectSettingsPage={requestPage}
              onRemoveProject={onRemoveProject}
              onUnarchiveConversation={onUnarchiveConversation}
              onUiPreferencesChange={onUiPreferencesChange}
              initialProjectId={initialProjectId}
              initialBrowserView={browserEntryView}
              browserPageRevision={browserPageRevision}
              onCloseSettings={onBack}
              uiPreferences={uiPreferences}
            />
          </SettingsSearchNavigationProvider>
        </div>
      </main>
      {pendingNavigation && (
        <ConfirmationDialog
          cancelLabel={t('mcp.actions.cancel')}
          confirmLabel={t('mcp.unsaved.discard')}
          description={t('mcp.unsaved.description')}
          onCancel={() => setPendingNavigation(null)}
          onConfirm={confirmPendingNavigation}
          title={t('mcp.unsaved.title')}
        />
      )}
    </div>
  )
}
