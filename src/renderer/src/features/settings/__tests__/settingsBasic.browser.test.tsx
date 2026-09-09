import type { ComponentType } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getTranslation } from '../../../config/frontendTranslations'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import { buildSettingsSearchIndex, searchSettings } from '../settingsSearch'
import { SettingsSearchNavigationProvider } from '../settingsSearchNavigation'
import { generalSettingsNodes } from '../pages/GeneralSettingsPage.definition'
import { appearanceSettingsNodes } from '../pages/AppearanceSettingsPage.definition'
import { profileSettingsNodes } from '../pages/ProfileSettingsPage.definition'
import { personalizationSettingsNodes } from '../pages/PersonalizationSettingsPage.definition'
import { usageBillingSettingsNodes } from '../pages/UsageBillingSettingsPage.definition'
import { GeneralSettingsPage } from '../pages/GeneralSettingsPage'
import { AppearanceSettingsPage } from '../pages/AppearanceSettingsPage'
import { ProfileSettingsPage } from '../pages/ProfileSettingsPage'
import { PersonalizationSettingsPage } from '../pages/PersonalizationSettingsPage'
import { UsageBillingSettingsPage } from '../pages/UsageBillingSettingsPage'

const mocks = vi.hoisted(() => ({ mac: true, notifications: true, update: vi.fn() }))

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/frontendTranslations')
  const { defaultThemeIdsByColorScheme } = await import('../../../config/frontendTheme')
  const config = {
    language: 'zh-CN',
    t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key),
    setLanguage: vi.fn(),
    colorSchemePreference: 'system',
    resolvedThemeId: defaultThemeIdsByColorScheme.light,
    themeIdsByColorScheme: defaultThemeIdsByColorScheme,
    setColorSchemePreference: vi.fn(),
    setThemeForColorScheme: vi.fn()
  }
  return { useFrontendConfig: () => config }
})
vi.mock('../../../host/hostClient', () => ({
  hostClient: { agent: { onPromptPreferencesChanged: vi.fn(() => () => undefined) } }
}))
vi.mock('../../../lib/platform', () => ({ isMacOS: () => mocks.mac }))
vi.mock('../../notifications/notificationClient', () => ({
  hasNotificationHostApi: () => mocks.notifications
}))
vi.mock('../../notifications/useNotificationSettings', () => ({
  useNotificationSettings: () => ({
    settings: {
      enabled: false,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false,
      soundEnabled: false,
      showTaskContent: false
    },
    status: 'ready',
    error: null,
    refresh: vi.fn(),
    update: mocks.update,
    saving: false
  })
}))
vi.mock('../pages/useHumanInteractionSettings', () => ({
  useHumanInteractionSettings: () => ({
    settings: { enabled: true, revision: 1 },
    loading: false,
    saving: false,
    error: null,
    refresh: vi.fn(),
    toggle: vi.fn()
  })
}))
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [] })
}))
vi.mock('../../storage/storageClient', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../storage/storageClient')>()
  return {
    ...actual,
    loadAgentPromptPreferences: vi.fn(async () => actual.defaultAgentPromptPreferences()),
    saveAgentPromptPreferences: vi.fn()
  }
})
vi.mock('../../agent/agentClient', () => ({
  getAgentUsageSummary: vi.fn(async () => ({ models: [] })),
  clearAgentUsageRecords: vi.fn()
}))

const preferences: UiPreferencesSnapshot = {
  profileAvatarDataUrl: null,
  profileDisplayName: '长中文设置名称',
  profileHandle: 'USER',
  sidebarConversationSort: 'updated',
  sidebarProjectSort: 'created',
  sidebarProjectOrder: [],
  sidebarSectionOrder: 'projects_first',
  nativeFontSmoothing: false,
  showTokenUsageDetails: true,
  showContextWindowUsage: true,
  translucentSidebar: true,
  translucentSidebarTransparency: 54,
  fullPermissionEnabled: true,
  customPermissionEnabled: true,
  customPermissions: {
    read: 'workspace_only',
    write: 'workspace_only',
    command: 'require_approval',
    commandSafety: 'guarded',
    patch: 'require_approval',
    builtinExecution: 'require_approval'
  },
  updatedAt: 0
}
const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
type BasicPage = ComponentType<{
  uiPreferences: UiPreferencesSnapshot
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
}>
const pages = [
  {
    id: 'general',
    labelKey: 'settings.page.general',
    nodes: generalSettingsNodes,
    Page: GeneralSettingsPage
  },
  {
    id: 'appearance',
    labelKey: 'settings.page.appearance',
    nodes: appearanceSettingsNodes,
    Page: AppearanceSettingsPage
  },
  {
    id: 'profile',
    labelKey: 'settings.page.profile',
    nodes: profileSettingsNodes,
    Page: ProfileSettingsPage
  },
  {
    id: 'personalization',
    labelKey: 'settings.page.personalization',
    nodes: personalizationSettingsNodes,
    Page: PersonalizationSettingsPage
  },
  {
    id: 'usageBilling',
    labelKey: 'settings.page.usageBilling',
    nodes: usageBillingSettingsNodes,
    Page: UsageBillingSettingsPage
  }
] as const

beforeEach(() => {
  mocks.mac = true
  mocks.notifications = true
  mocks.update.mockReset()
})

describe('basic settings definitions and real page targets', () => {
  for (const definition of pages) {
    it(`${definition.id} renders one target for every indexed setting`, async () => {
      const Page: BasicPage = definition.Page
      const screen = await render(
        <Page uiPreferences={preferences} onUiPreferencesChange={vi.fn()} />
      )
      for (const result of buildSettingsSearchIndex([definition], t)) {
        const targets = screen.container.querySelectorAll(`[data-setting-id="${result.id}"]`)
        expect(targets.length, result.id).toBe(1)
        expect(targets[0].textContent, result.id).toContain(result.label)
      }
    })
  }

  it('keeps platform and host availability consistent between search and rendered controls', async () => {
    mocks.mac = false
    mocks.notifications = false
    const index = buildSettingsSearchIndex(pages, t)
    expect(index.some((entry) => entry.id === 'appearance.nativeFontSmoothing')).toBe(false)
    expect(index.some((entry) => entry.id.startsWith('notifications.'))).toBe(false)
    const screen = await render(
      <>
        <AppearanceSettingsPage uiPreferences={preferences} onUiPreferencesChange={vi.fn()} />
        <GeneralSettingsPage uiPreferences={preferences} onUiPreferencesChange={vi.fn()} />
      </>
    )
    expect(
      screen.container.querySelector('[data-setting-id="appearance.nativeFontSmoothing"]')
    ).toBeNull()
    expect(screen.container.querySelector('[data-setting-id="notifications"]')).toBeNull()
  })

  it('finds the actual theme selector through its existing system option label', () => {
    const index = buildSettingsSearchIndex(pages, t)
    expect(
      searchSettings(index, t('appearance.theme.system')).some(
        (entry) => entry.id === 'appearance.theme'
      )
    ).toBe(true)
  })

  it('reveals the existing custom notification drawer without saving any notification preference', async () => {
    const changePreferences = vi.fn()
    const screen = await render(
      <SettingsSearchNavigationProvider
        target={{
          page: 'general',
          id: 'notifications.humanCompletedEnabled',
          view: 'notificationCustom',
          revision: 1
        }}
      >
        <GeneralSettingsPage
          uiPreferences={preferences}
          onUiPreferencesChange={changePreferences}
        />
      </SettingsSearchNavigationProvider>
    )
    const target = screen.container.querySelector(
      '[data-setting-id="notifications.humanCompletedEnabled"]'
    )!
    expect(target.closest('.general-notification-drawer')?.getAttribute('data-open')).toBe('true')
    expect(target.querySelector('[role="switch"]')?.getAttribute('aria-checked')).toBe('false')
    expect(mocks.update).not.toHaveBeenCalled()
    expect(changePreferences).not.toHaveBeenCalled()
  })
})
