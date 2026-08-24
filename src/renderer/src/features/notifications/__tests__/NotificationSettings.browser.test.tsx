import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { defaultUiPreferences } from '../../storage/storageClient'
import { GeneralSettingsPage } from '../../settings/pages/GeneralSettingsPage'
import '../../settings/pages/GeneralSettingsPage.css'

const mocks = vi.hoisted(() => ({
  update: vi.fn(async () => undefined),
  useNotificationSettings: vi.fn()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'en-US',
    setLanguage: vi.fn(),
    t: (key: string) => key
  })
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

vi.mock('../notificationClient', () => ({
  hasNotificationHostApi: () => true
}))

vi.mock('../useNotificationSettings', () => ({
  useNotificationSettings: mocks.useNotificationSettings
}))

describe('notification settings', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.useNotificationSettings.mockReturnValue({
      settings: {
        schemaVersion: 1,
        enabled: true,
        soundEnabled: true,
        showTaskContent: true,
        humanCompletedEnabled: true,
        humanFailedEnabled: true,
        humanApprovalEnabled: true,
        humanCancelledEnabled: true,
        revision: 1,
        updatedAt: 1
      },
      status: 'ready',
      error: null,
      refresh: vi.fn(),
      update: mocks.update,
      saving: false
    })
  })

  it('uses one standard settings section for sound, privacy, and ordinary task states', async () => {
    const screen = await render(
      <GeneralSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={defaultUiPreferences()} />
    )

    await expect
      .element(screen.getByRole('heading', { name: 'general.sectionNotifications' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingSound' }))
      .toBeChecked()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingPreview' }))
      .toBeChecked()

    await screen.getByRole('switch', { name: 'notification.settingApproval' }).click()
    expect(mocks.update).toHaveBeenCalledWith({ humanApprovalEnabled: false })
  })

  it('disables dependent settings when system notifications are disabled', async () => {
    mocks.useNotificationSettings.mockReturnValue({
      ...mocks.useNotificationSettings(),
      settings: { ...mocks.useNotificationSettings().settings, enabled: false }
    })
    const screen = await render(
      <GeneralSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={defaultUiPreferences()} />
    )

    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingEnabled' }))
      .not.toBeChecked()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingSound' }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingApproval' }))
      .toBeDisabled()
  })
})
