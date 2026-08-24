import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { NotificationSettings } from '@mycopilot/protocol'
import { defaultUiPreferences } from '../../storage/storageClient'
import { GeneralSettingsPage } from '../../settings/pages/GeneralSettingsPage'
import '../../settings/pages/GeneralSettingsPage.css'

const mocks = vi.hoisted(() => ({
  refresh: vi.fn(async () => undefined),
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

const readySettings = (patch: Partial<NotificationSettings> = {}): NotificationSettings => ({
  schemaVersion: 1,
  enabled: true,
  soundEnabled: true,
  showTaskContent: true,
  humanCompletedEnabled: true,
  humanFailedEnabled: true,
  humanApprovalEnabled: true,
  humanCancelledEnabled: true,
  revision: 1,
  updatedAt: 1,
  ...patch
})

function mockReadySettings(patch: Partial<NotificationSettings> = {}) {
  mocks.useNotificationSettings.mockReturnValue({
    settings: readySettings(patch),
    status: 'ready',
    error: null,
    refresh: mocks.refresh,
    update: mocks.update,
    saving: false
  })
}

async function renderGeneralSettings() {
  return render(
    <GeneralSettingsPage onUiPreferencesChange={vi.fn()} uiPreferences={defaultUiPreferences()} />
  )
}

describe('notification settings', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockReadySettings()
  })

  it('keeps notifications as the final General section and presentation settings as its final rows', async () => {
    const screen = await renderGeneralSettings()
    const heading = screen
      .getByRole('heading', { name: 'general.sectionNotifications' })
      .element() as HTMLElement
    const section = heading.closest('section')
    const article = heading.closest('article')
    const rows = Array.from(
      section?.querySelectorAll(
        ':scope > .general-notification-settings-list > .settings-list-row'
      ) ?? []
    )

    expect(section).toBe(article?.lastElementChild)
    expect(rows.at(-2)?.textContent).toContain('notification.settingSound')
    expect(rows.at(-1)?.textContent).toContain('notification.settingPreview')
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingSound' }))
      .toBeEnabled()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingPreview' }))
      .toBeEnabled()
  })

  it('derives the necessary preset and renders its matching description', async () => {
    mockReadySettings({ humanCompletedEnabled: false })
    const screen = await renderGeneralSettings()

    await expect
      .element(screen.getByRole('button', { name: 'notification.ordinaryModeAria' }))
      .toHaveTextContent('notification.ordinaryModeNecessary')
    await expect
      .element(screen.getByText('notification.ordinaryModeNecessaryDescription'))
      .toBeVisible()
  })

  it('opens the four-state custom drawer even when the stored flags match a preset', async () => {
    const screen = await renderGeneralSettings()
    const modeButton = screen.getByRole('button', { name: 'notification.ordinaryModeAria' })

    await modeButton.click()
    await screen.getByRole('option', { name: 'notification.ordinaryModeCustom' }).click()

    await expect.element(modeButton).toHaveTextContent('notification.ordinaryModeCustom')
    await expect
      .element(screen.getByRole('group', { name: 'notification.ordinaryCustomAria' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingCompleted' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingFailed' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingApproval' }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: 'notification.settingCancelled' }))
      .toBeVisible()

    await screen.getByRole('switch', { name: 'notification.settingApproval' }).click()
    expect(mocks.update).toHaveBeenCalledWith({
      enabled: true,
      humanApprovalEnabled: false
    })
  })

  it('maps the necessary preset to the four ordinary-task flags', async () => {
    mockReadySettings({
      humanCompletedEnabled: true,
      humanFailedEnabled: false,
      humanApprovalEnabled: true,
      humanCancelledEnabled: false
    })
    const screen = await renderGeneralSettings()
    const modeButton = screen.getByRole('button', { name: 'notification.ordinaryModeAria' })

    await modeButton.click()
    await screen.getByRole('option', { name: 'notification.ordinaryModeNecessary' }).click()

    expect(mocks.update).toHaveBeenCalledWith({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: true,
      humanApprovalEnabled: true,
      humanCancelledEnabled: true
    })
  })

  it('turns ordinary task notifications off without disabling the shared automation pipeline', async () => {
    const screen = await renderGeneralSettings()
    const modeButton = screen.getByRole('button', { name: 'notification.ordinaryModeAria' })

    await modeButton.click()
    await screen.getByRole('option', { name: 'notification.ordinaryModeNever' }).click()

    expect(mocks.update).toHaveBeenCalledWith({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false
    })
  })

  it('keeps sound and task content available in never mode and normalizes legacy global-off on write', async () => {
    mockReadySettings({ enabled: false })
    const screen = await renderGeneralSettings()

    await expect
      .element(screen.getByRole('button', { name: 'notification.ordinaryModeAria' }))
      .toHaveTextContent('notification.ordinaryModeNever')
    const sound = screen.getByRole('switch', { name: 'notification.settingSound' })
    const preview = screen.getByRole('switch', { name: 'notification.settingPreview' })
    await expect.element(sound).toBeEnabled()
    await expect.element(preview).toBeEnabled()

    await sound.click()
    expect(mocks.update).toHaveBeenCalledWith({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false,
      soundEnabled: false
    })
  })

  it('rolls back an explicit custom selection if legacy global-off cannot be normalized', async () => {
    let rejectUpdate: ((reason: Error) => void) | undefined
    mocks.update.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          rejectUpdate = reject
        })
    )
    mockReadySettings({ enabled: false })
    const screen = await renderGeneralSettings()
    const modeButton = screen.getByRole('button', { name: 'notification.ordinaryModeAria' })

    await modeButton.click()
    await screen.getByRole('option', { name: 'notification.ordinaryModeCustom' }).click()
    await expect.element(modeButton).toHaveTextContent('notification.ordinaryModeCustom')
    expect(mocks.update).toHaveBeenCalledWith({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false
    })

    rejectUpdate?.(new Error('temporary RPC failure'))
    await expect.element(modeButton).toHaveTextContent('notification.ordinaryModeNever')
  })

  it('opens the mode menu upward from the final section without clipping it', async () => {
    const screen = await renderGeneralSettings()
    const modeButton = screen.getByRole('button', { name: 'notification.ordinaryModeAria' })

    await modeButton.click()
    const menu = screen.getByRole('listbox', { name: 'notification.ordinaryModeAria' })
    await expect.element(menu).toBeVisible()

    const buttonElement = modeButton.element() as HTMLElement
    const menuElement = menu.element() as HTMLElement
    const list = buttonElement.closest('.settings-list') as HTMLElement
    expect(menuElement.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      buttonElement.getBoundingClientRect().top
    )
    expect(getComputedStyle(list).overflow).toBe('visible')
  })
})
