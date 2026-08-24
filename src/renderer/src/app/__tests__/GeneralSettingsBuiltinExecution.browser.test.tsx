import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { zhCNTranslations } from '../../config/frontendTranslations.zhCN'
import { GeneralSettingsPage } from '../../features/settings/pages/GeneralSettingsPage'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'

const { setLanguage } = vi.hoisted(() => ({ setLanguage: vi.fn() }))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    setLanguage,
    t: (key: string) => key
  })
}))

const preferences: UiPreferencesSnapshot = {
  profileAvatarDataUrl: null,
  profileDisplayName: '',
  profileHandle: 'USER',
  sidebarConversationSort: 'updated',
  sidebarProjectSort: 'created',
  sidebarProjectOrder: [],
  sidebarSectionOrder: 'projects_first',
  nativeFontSmoothing: false,
  showTokenUsageDetails: true,
  showContextWindowUsage: true,
  translucentSidebar: false,
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

describe('GeneralSettingsPage built-in execution permission', () => {
  beforeEach(() => {
    setLanguage.mockReset()
  })

  it('lists every registered language and selects the requested locale', async () => {
    const screen = await render(
      <GeneralSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={preferences} />
    )

    await screen.getByRole('button', { name: 'general.languageAria' }).click()

    for (const label of [
      '中文（中国）',
      '繁體中文',
      'English (United States)',
      'English (United Kingdom)',
      '한국어',
      '日本語',
      'Français',
      'Italiano',
      'Русский'
    ]) {
      await expect.element(screen.getByRole('option', { name: label })).toBeVisible()
    }

    await screen.getByRole('option', { name: '日本語' }).click()
    expect(setLanguage).toHaveBeenCalledWith('ja-JP')
  })

  it('renders one custom-permission row with the exact Chinese copy and writes the toggle back', async () => {
    expect(zhCNTranslations['general.autoApproveBuiltinExecution']).toBe('内置能力无需审批')
    expect(zhCNTranslations['general.autoApproveBuiltinExecutionDescription']).toBe(
      '开启后，内置skill/插件的脚本和工具执行无需审批。'
    )

    const onUiPreferencesChange = vi.fn()
    const screen = await render(
      <GeneralSettingsPage
        onUiPreferencesChange={onUiPreferencesChange}
        uiPreferences={preferences}
      />
    )

    const permissionToggle = screen.getByRole('switch', {
      name: 'general.autoApproveBuiltinExecution'
    })
    await expect.element(permissionToggle).toHaveAttribute('aria-checked', 'false')
    const toggleElement = permissionToggle.element()
    expect(
      document.querySelectorAll('[aria-label="general.autoApproveBuiltinExecution"]')
    ).toHaveLength(1)
    expect(toggleElement.closest('section')?.getAttribute('aria-labelledby')).toBe(
      'custom-permissions-heading'
    )

    await permissionToggle.click()
    expect(onUiPreferencesChange).toHaveBeenCalledWith({
      customPermissions: {
        ...preferences.customPermissions,
        builtinExecution: 'auto_approve'
      }
    })
  })
})
