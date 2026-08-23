import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { zhCNTranslations } from '../../config/frontendTranslations.zhCN'
import { GeneralSettingsPage } from '../../features/settings/pages/GeneralSettingsPage'
import type { UiPreferencesSnapshot } from '../../features/storage/storageClient'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    setLanguage: vi.fn(),
    t: (key: string) => key
  })
}))

describe('GeneralSettingsPage built-in execution permission', () => {
  it('renders one custom-permission row with the exact Chinese copy and writes the toggle back', async () => {
    expect(zhCNTranslations['general.autoApproveBuiltinExecution']).toBe('内置能力无需审批')
    expect(zhCNTranslations['general.autoApproveBuiltinExecutionDescription']).toBe(
      '开启后，内置skill/插件的脚本和工具执行无需审批。'
    )

    const onUiPreferencesChange = vi.fn()
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
