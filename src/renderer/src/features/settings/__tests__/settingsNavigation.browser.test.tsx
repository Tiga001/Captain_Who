import { act } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getTranslation } from '../../../config/frontendTranslations'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import { SettingsPage, type SettingsPageId } from '../SettingsPage'
import '../../../styles/global.css'

const mocks = vi.hoisted(() => ({
  update: vi.fn(),
  configurationMounts: 0,
  revealConfiguration: undefined as (() => void) | undefined
}))
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
    setThemeForColorScheme: vi.fn(),
    uiContrast: 0,
    setUiContrast: vi.fn()
  }
  return { useFrontendConfig: () => config }
})
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../lib/platform', () => ({ isMacOS: () => true }))
vi.mock('../../notifications/notificationClient', () => ({
  hasNotificationHostApi: () => false
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

vi.mock('../pages/ConfigurationSettingsPage', async () => {
  const { useEffect, useState } = await import('react')
  return {
    ConfigurationSettingsPage: () => {
      const [shown, setShown] = useState(false)
      useEffect(() => {
        mocks.configurationMounts += 1
        mocks.revealConfiguration = () => setShown(true)
        return () => {
          mocks.revealConfiguration = undefined
        }
      }, [])
      return (
        <article>
          <h1>配置</h1>
          <div style={{ height: 1000 }} />
          {shown && (
            <>
              <div data-setting-id="configuration.apiUrl" style={{ height: 48 }}>
                API URL
              </div>
              <div style={{ height: 800 }} />
              <div data-setting-id="configuration.apiToken" style={{ height: 48 }}>
                API Token
              </div>
            </>
          )}
          <div style={{ height: 1000 }} />
        </article>
      )
    }
  }
})
vi.mock('../../mcp/McpSettingsPage', async () => {
  const { useEffect } = await import('react')
  return {
    McpSettingsPage: ({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) => {
      useEffect(() => {
        onDirtyChange(true)
        return () => onDirtyChange(false)
      }, [onDirtyChange])
      return (
        <article>
          <h1>MCP 草稿</h1>
          <textarea aria-label="草稿内容" defaultValue="尚未保存" />
        </article>
      )
    }
  }
})
vi.mock('../../mcp/BrowserAutomationSettingsPage', () => ({
  BrowserAutomationSettingsPage: () => null
}))
vi.mock('../pages/EnvironmentSettingsPage', () => ({ EnvironmentSettingsPage: () => null }))
vi.mock('../pages/SkillsSettingsPage', () => ({ SkillsSettingsPage: () => null }))
vi.mock('../pages/AgentTemplatesSettingsPage', () => ({ AgentTemplatesSettingsPage: () => null }))
vi.mock('../pages/ArchivedConversationsSettingsPage', () => ({
  ArchivedConversationsSettingsPage: () => null
}))
vi.mock('../pages/PersonalizationSettingsPage', () => ({ PersonalizationSettingsPage: () => null }))
vi.mock('../pages/UsageBillingSettingsPage', () => ({ UsageBillingSettingsPage: () => null }))

const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
const search = () => page.getByRole('searchbox', { name: t('settings.search') })
const content = () => document.querySelector<HTMLElement>('.settings-content')!
const rightPage = () => content().querySelector('h1')?.textContent
const resultButtons = () => [
  ...document.querySelectorAll<HTMLButtonElement>('[data-setting-result]')
]
function result(label: string, pageLabel?: string) {
  const button = resultButtons().find(
    (button) =>
      button.querySelector('.settings-nav__result-title')?.textContent === label &&
      (!pageLabel || button.closest('section')?.getAttribute('aria-label') === pageLabel)
  )
  expect(button, `search result ${pageLabel ?? ''} / ${label}`).toBeDefined()
  return button!
}
async function frames() {
  await act(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
  })
}
async function click(button: HTMLElement) {
  await act(async () => button.click())
  await frames()
}
async function key(value: string, isComposing = false) {
  await act(async () =>
    document
      .querySelector<HTMLInputElement>('.settings-nav__search input')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true, isComposing }))
  )
  await frames()
}
async function setup(initialPage: SettingsPageId = 'general') {
  const changed = vi.fn()
  const screen = await render(
    <div style={getFrontendCssVariables()}>
      <SettingsPage
        initialPage={initialPage}
        conversations={[]}
        projects={[]}
        onBack={vi.fn()}
        onDeleteArchivedConversations={vi.fn()}
        onDeleteConversation={vi.fn()}
        onRemoveProject={vi.fn(async () => true)}
        onUnarchiveConversation={vi.fn()}
        onUiPreferencesChange={changed}
        uiPreferences={preferences}
      />
    </div>
  )
  await frames()
  return { screen, changed }
}

beforeEach(async () => {
  mocks.configurationMounts = 0
  mocks.revealConfiguration = undefined
  mocks.update.mockClear()
  await page.viewport(1200, 600)
})

describe('settings sidebar search navigation', () => {
  it('finds a setting on an unvisited page without switching or saving while typing', async () => {
    const { changed } = await setup('profile')
    const original = content().innerHTML
    await search().fill('  api   TOKEN ')
    expect(result('API Token')).toBeDefined()
    expect(mocks.configurationMounts).toBe(0)
    expect(content().innerHTML).toBe(original)
    expect(changed).not.toHaveBeenCalled()
    expect(mocks.update).not.toHaveBeenCalled()
  })

  it('scrolls a real setting repeatedly while preserving the right-side DOM and styles', async () => {
    const { changed } = await setup('general')
    const original = content().innerHTML
    await search().fill(t('general.autoApproveCommands'))
    const button = result(t('general.autoApproveCommands'), t('settings.page.general'))
    await click(button)
    expect(content().scrollTop).toBeGreaterThan(0)
    expect(content().innerHTML).toBe(original)
    content().scrollTop = 0
    await click(button)
    expect(content().scrollTop).toBeGreaterThan(0)
    expect(content().innerHTML).toBe(original)
    expect(changed).not.toHaveBeenCalled()
  })

  it('keeps IME confirmation on the input and restores normal navigation after Escape', async () => {
    await setup('profile')
    await search().fill(t('appearance.translucentSidebar'))
    await key('Enter', true)
    expect(rightPage()).toBe(t('settings.page.profile'))
    await key('Escape')
    expect(document.querySelector<HTMLInputElement>('.settings-nav__search input')!.value).toBe('')
    expect(document.querySelector('.settings-nav__results')).toBeNull()
    expect(rightPage()).toBe(t('settings.page.profile'))
    await search().fill('不存在的设置 qzx998')
    expect(document.querySelector('.settings-nav__empty')?.textContent).toContain(
      t('settings.search.empty')
    )
  })

  it('Enter selects the first visible ranked result across page groups', async () => {
    await setup('profile')
    await search().fill('模型')
    const first = resultButtons()[0]
    expect(first).toBeDefined()
    expect(document.querySelectorAll('.settings-nav__result-group').length).toBeGreaterThan(1)
    await key('Enter')
    expect(first.getAttribute('aria-current')).toBe('location')
    expect(
      resultButtons().filter((button) => button.getAttribute('aria-current') === 'location')
    ).toEqual([first])
  })

  for (const clearMode of ['button', 'Escape', 'empty input', 'whitespace input'] as const) {
    it(`cancels delayed target scrolling when cleared with ${clearMode}`, async () => {
      await setup('profile')
      await search().fill('API Token')
      await click(result('API Token'))
      expect(mocks.configurationMounts).toBe(1)
      if (clearMode === 'button')
        await page.getByRole('button', { name: t('settings.search.clear') }).click()
      else if (clearMode === 'Escape') await key('Escape')
      else await search().fill(clearMode === 'empty input' ? '' : '   ')
      content().scrollTop = 77
      await act(async () => mocks.revealConfiguration?.())
      await frames()
      expect(content().scrollTop).toBe(77)
      expect(document.querySelector('.settings-nav__results')).toBeNull()
    })
  }

  it('only scrolls to the latest selected setting when delayed content arrives', async () => {
    await setup('profile')
    await search().fill('API')
    await click(result('API Token'))
    await click(result('API URL'))
    await act(async () => mocks.revealConfiguration?.())
    await frames()
    const url = content().querySelector<HTMLElement>('[data-setting-id="configuration.apiUrl"]')!
    expect(
      Math.abs(url.getBoundingClientRect().top - content().getBoundingClientRect().top - 24)
    ).toBeLessThan(2)
    expect(result('API URL').getAttribute('aria-current')).toBe('location')
    expect(result('API Token').hasAttribute('aria-current')).toBe(false)
  })

  it('preserves an MCP draft on cancel and resumes the exact setting target after discard', async () => {
    await setup('mcp')
    await search().fill(t('profile.handle'))
    await click(result(t('profile.handle'), t('settings.page.profile')))
    expect(document.querySelector('[role="dialog"]')).not.toBeNull()
    expect(rightPage()).toBe('MCP 草稿')
    await click(document.querySelector<HTMLElement>('.app-confirm-dialog__button--cancel')!)
    expect(rightPage()).toBe('MCP 草稿')
    expect(document.querySelector<HTMLTextAreaElement>('[aria-label="草稿内容"]')!.value).toBe(
      '尚未保存'
    )
    await click(result(t('profile.handle'), t('settings.page.profile')))
    await click(document.querySelector<HTMLElement>('.app-confirm-dialog__button--danger')!)
    expect(rightPage()).toBe(t('settings.page.profile'))
    expect(
      result(t('profile.handle'), t('settings.page.profile')).getAttribute('aria-current')
    ).toBe('location')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })
})
