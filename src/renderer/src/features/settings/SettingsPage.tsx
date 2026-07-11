import { useEffect, useState, type CSSProperties } from 'react'
import type { LucideIcon } from 'lucide-react'
import {
  Archive,
  ArrowLeft,
  Clock,
  Gauge,
  Monitor,
  Search,
  Settings,
  Shield,
  Sun,
  UserCircle
} from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { isMacOS } from '../../lib/platform'
import type { AppProject } from '../../config/projectConfig'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { ChatConversation } from '../chat/chatTypes'
import type { UiPreferencesSnapshot } from '../storage/storageClient'
import { getTranslucentSidebarOpacityPercent } from '../storage/storageClient'
import { AppearanceSettingsPage } from './pages/AppearanceSettingsPage'
import { ArchivedConversationsSettingsPage } from './pages/ArchivedConversationsSettingsPage'
import { ConfigurationSettingsPage } from './pages/ConfigurationSettingsPage'
import { EnvironmentSettingsPage } from './pages/EnvironmentSettingsPage'
import { GeneralSettingsPage } from './pages/GeneralSettingsPage'
import { PersonalizationSettingsPage } from './pages/PersonalizationSettingsPage'
import { ProfileSettingsPage } from './pages/ProfileSettingsPage'
import { UsageBillingSettingsPage } from './pages/UsageBillingSettingsPage'
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
  initialPage?: SettingsPageId
  uiPreferences: UiPreferencesSnapshot
}

export type SettingsPageId =
  | 'general'
  | 'profile'
  | 'appearance'
  | 'configuration'
  | 'personalization'
  | 'usageBilling'
  | 'environment'
  | 'archivedConversations'

interface SettingsNavItem {
  id: SettingsPageId
  labelKey: TranslationKey
  icon: LucideIcon
}

const SETTINGS_GROUPS: Array<{ titleKey: TranslationKey; items: SettingsNavItem[] }> = [
  {
    titleKey: 'settings.group.personal',
    items: [
      { id: 'general', labelKey: 'settings.page.general', icon: Settings },
      { id: 'profile', labelKey: 'settings.page.profile', icon: UserCircle },
      { id: 'appearance', labelKey: 'settings.page.appearance', icon: Sun },
      { id: 'configuration', labelKey: 'settings.page.configuration', icon: Shield },
      { id: 'personalization', labelKey: 'settings.page.personalization', icon: Clock },
      { id: 'usageBilling', labelKey: 'settings.page.usageBilling', icon: Gauge }
    ]
  },
  {
    titleKey: 'settings.group.coding',
    items: [{ id: 'environment', labelKey: 'settings.page.environment', icon: Monitor }]
  },
  {
    titleKey: 'settings.group.archived',
    items: [
      {
        id: 'archivedConversations',
        labelKey: 'settings.page.archivedConversations',
        icon: Archive
      }
    ]
  }
]

function SettingsContent({
  activePage,
  conversations,
  onDeleteArchivedConversations,
  onDeleteConversation,
  onRemoveProject,
  onUnarchiveConversation,
  onUiPreferencesChange,
  projects,
  uiPreferences
}: {
  activePage: SettingsPageId
  conversations: ChatConversation[]
  onDeleteArchivedConversations: (conversationIds: string[]) => void
  onDeleteConversation: (conversationId: string) => void
  onRemoveProject: (projectId: string) => Promise<boolean>
  onUnarchiveConversation: (conversationId: string) => void
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
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
    return <ConfigurationSettingsPage />
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
}

function SettingsNavigation({ activePage, onBack, onSelectPage }: SettingsNavigationProps) {
  const { t } = useFrontendConfig()
  const [searchQuery, setSearchQuery] = useState('')
  const normalizedSearchQuery = searchQuery.trim().toLowerCase()
  const visibleGroups = SETTINGS_GROUPS.map((group) => ({
    ...group,
    items: normalizedSearchQuery
      ? group.items.filter((item) => {
          const label = t(item.labelKey).toLowerCase()
          const groupTitle = t(group.titleKey).toLowerCase()
          return label.includes(normalizedSearchQuery) || groupTitle.includes(normalizedSearchQuery)
        })
      : group.items
  })).filter((group) => group.items.length > 0)

  return (
    <aside className="settings-nav" aria-label={t('settings.navigation')}>
      <button className="settings-nav__back" type="button" onClick={onBack}>
        <ArrowLeft aria-hidden="true" />
        <span>{t('settings.backToApp')}</span>
      </button>

      <label className="settings-nav__search">
        <Search aria-hidden="true" />
        <input
          type="search"
          placeholder={t('settings.searchPlaceholder')}
          aria-label={t('settings.search')}
          value={searchQuery}
          onChange={(event) => setSearchQuery(event.currentTarget.value)}
        />
      </label>

      <nav className="settings-nav__groups">
        {visibleGroups.map((group) => (
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
  initialPage = 'general',
  uiPreferences
}: SettingsPageProps) {
  const { t } = useFrontendConfig()
  const [activePage, setActivePage] = useState<SettingsPageId>(initialPage)

  useEffect(() => {
    setActivePage(initialPage)
  }, [initialPage])

  return (
    <div
      className="settings-page"
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
      <SettingsNavigation activePage={activePage} onBack={onBack} onSelectPage={setActivePage} />

      <main className="settings-content" aria-label={t('settings.content')}>
        <div className="settings-content__inner">
          <SettingsContent
            activePage={activePage}
            conversations={conversations}
            projects={projects}
            onDeleteArchivedConversations={onDeleteArchivedConversations}
            onDeleteConversation={onDeleteConversation}
            onRemoveProject={onRemoveProject}
            onUnarchiveConversation={onUnarchiveConversation}
            onUiPreferencesChange={onUiPreferencesChange}
            uiPreferences={uiPreferences}
          />
        </div>
      </main>
    </div>
  )
}
