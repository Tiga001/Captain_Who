import type { HumanInteractionSettings } from '@mycopilot/protocol'
import type { HostInvocationResult } from '@mycopilot/host-api'
import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import '../../features/settings/SettingsPage.css'

const service = vi.hoisted(() => ({
  get: vi.fn(),
  update: vi.fn(),
  subscribe: vi.fn(),
  unsubscribe: vi.fn(),
  subscribeResync: vi.fn(),
  unsubscribeResync: vi.fn(),
  loadPreferences: vi.fn(),
  savePreferences: vi.fn(),
  listRequests: vi.fn(),
  submit: vi.fn(),
  ignore: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    humanInteraction: {
      getSettings: service.get,
      updateSettings: service.update,
      onSettingsChanged: service.subscribe,
      onResync: service.subscribeResync,
      listRequests: service.listRequests,
      submit: service.submit,
      ignore: service.ignore
    }
  }
}))
vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/frontendTranslations')
  const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
  return { useFrontendConfig: () => ({ t }) }
})
vi.mock('../../features/storage/storageClient', () => ({
  defaultAgentPromptPreferences: () => ({
    workMode: 'coding',
    tone: 'pragmatic',
    detailLevel: 'medium',
    customInstructions: '',
    updatedAt: 0
  }),
  loadAgentPromptPreferences: service.loadPreferences,
  saveAgentPromptPreferences: service.savePreferences
}))

const { PersonalizationSettingsPage } =
  await import('../../features/settings/pages/PersonalizationSettingsPage')
const { HumanInteractionSettingsSection } =
  await import('../../features/settings/pages/HumanInteractionSettingsSection')
const label = '允许智能体向人类提问'
let changed: (settings: HumanInteractionSettings) => void
let resync: () => void

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((yes, no) => {
    resolve = yes
    reject = no
  })
  return { promise, resolve, reject }
}
function snapshot(enabled = true, revision = 0): HumanInteractionSettings {
  return { enabled, revision, updatedAt: revision * 10 }
}
function ok(value: HumanInteractionSettings): HostInvocationResult<HumanInteractionSettings> {
  return { ok: true, value }
}

beforeEach(() => {
  for (const method of Object.values(service)) method.mockReset()
  service.get.mockResolvedValue(ok(snapshot()))
  service.subscribe.mockImplementation((handler) => {
    changed = handler
    return service.unsubscribe
  })
  service.subscribeResync.mockImplementation((handler) => {
    resync = handler
    return service.unsubscribeResync
  })
  service.loadPreferences.mockResolvedValue({
    workMode: 'coding',
    tone: 'pragmatic',
    detailLevel: 'medium',
    customInstructions: 'Saved instructions',
    updatedAt: 1
  })
})

describe('Human interaction personalization settings', () => {
  it('uses Host settings and keeps a newer notification when the initial read arrives late', async () => {
    const pending = deferred<HostInvocationResult<HumanInteractionSettings>>()
    service.get.mockReturnValue(pending.promise)
    const screen = await render(<HumanInteractionSettingsSection />)
    await expect.element(screen.getByRole('status')).toHaveTextContent('正在读取设置')
    expect(screen.getByRole('switch').elements()).toHaveLength(0)
    changed(snapshot(false, 3))
    pending.resolve(ok(snapshot(true, 0)))
    const toggle = screen.getByRole('switch', { name: label })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    await expect.element(toggle).toBeEnabled()
    expect(service.get).toHaveBeenCalledWith({})
  })

  it('saves immediately with CAS while preserving unsaved prompt preferences and existing questions', async () => {
    const pending = deferred<HostInvocationResult<HumanInteractionSettings>>()
    service.update.mockReturnValue(pending.promise)
    const screen = await render(<PersonalizationSettingsPage />)
    const textarea = screen.getByRole('textbox')
    await expect.element(textarea).toHaveValue('Saved instructions')
    await textarea.fill('Unsaved draft with user changes')
    const toggle = screen.getByRole('switch', { name: label })
    await toggle.click()
    expect(service.update).toHaveBeenCalledExactlyOnceWith({ enabled: false, expectedRevision: 0 })
    await expect.element(toggle).toBeDisabled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    pending.resolve(ok(snapshot(false, 1)))
    await expect.element(toggle).toBeEnabled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    changed(snapshot(true, 2))
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    await expect.element(textarea).toHaveValue('Unsaved draft with user changes')
    await expect.element(screen.getByRole('button', { name: '保存', exact: true })).toBeEnabled()
    expect(service.loadPreferences).toHaveBeenCalledTimes(1)
    expect(service.savePreferences).not.toHaveBeenCalled()
    expect(service.listRequests).not.toHaveBeenCalled()
    expect(service.submit).not.toHaveBeenCalled()
    expect(service.ignore).not.toHaveBeenCalled()
  })

  it('accepts a settings notification even when an older read later fails', async () => {
    const pending = deferred<HostInvocationResult<HumanInteractionSettings>>()
    service.get.mockReturnValue(pending.promise)
    const screen = await render(<HumanInteractionSettingsSection />)
    changed(snapshot(false, 3))
    const toggle = screen.getByRole('switch', { name: label })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    await expect.element(toggle).toBeEnabled()
    pending.reject(new Error('old read lost its connection'))
    await expect.element(toggle).toBeEnabled()
    expect(screen.getByRole('alert').elements()).toHaveLength(0)
  })

  it.each(['rpc', 'transport'] as const)(
    'retains the authoritative value after a %s failure and reloads the revision before retry',
    async (failure) => {
      service.update.mockImplementationOnce(() =>
        failure === 'transport'
          ? Promise.reject(new Error('connection lost'))
          : Promise.resolve({ ok: false, error: { message: 'revision conflict' } })
      )
      const screen = await render(<HumanInteractionSettingsSection />)
      const toggle = screen.getByRole('switch', { name: label })
      await toggle.click()
      await expect.element(screen.getByRole('alert')).toHaveTextContent('原设置已保留')
      await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
      await expect.element(toggle).toBeEnabled()
      service.get.mockResolvedValue(ok(snapshot(false, 4)))
      await screen.getByRole('button', { name: '重新读取' }).click()
      await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
      service.update.mockResolvedValue(ok(snapshot(true, 5)))
      await toggle.click()
      expect(service.update).toHaveBeenLastCalledWith({ enabled: true, expectedRevision: 4 })
      await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    }
  )

  it('does not regress a newer notification when a save response or stale notification arrives', async () => {
    const pending = deferred<HostInvocationResult<HumanInteractionSettings>>()
    service.update.mockReturnValue(pending.promise)
    const screen = await render(<HumanInteractionSettingsSection />)
    const toggle = screen.getByRole('switch', { name: label })
    await toggle.click()
    changed(snapshot(true, 2))
    pending.resolve(ok(snapshot(false, 1)))
    await expect.element(toggle).toBeEnabled()
    changed(snapshot(false, 1))
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    service.update.mockResolvedValue(ok(snapshot(false, 3)))
    await toggle.click()
    expect(service.update).toHaveBeenLastCalledWith({ enabled: false, expectedRevision: 2 })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
  })

  it('offers a real reload after a failed initial read and unsubscribes on unmount', async () => {
    service.get.mockRejectedValueOnce(new Error('Core unavailable'))
    const screen = await render(<HumanInteractionSettingsSection />)
    await expect.element(screen.getByRole('alert')).toHaveTextContent('无法读取人机交互设置')
    expect(screen.getByRole('switch').elements()).toHaveLength(0)
    await screen.getByRole('button', { name: '重新读取' }).click()
    await expect
      .element(screen.getByRole('switch', { name: label }))
      .toHaveAttribute('aria-checked', 'true')
    await screen.unmount()
    expect(service.unsubscribe).toHaveBeenCalledTimes(1)
    expect(service.unsubscribeResync).toHaveBeenCalledTimes(1)
    changed(snapshot(false, 9))
  })

  it('refreshes from Host on focus and Core reconnect without reloading a personalization draft', async () => {
    const screen = await render(<PersonalizationSettingsPage />)
    const textarea = screen.getByRole('textbox')
    await expect.element(textarea).toHaveValue('Saved instructions')
    await textarea.fill('Keep this draft')
    service.get.mockResolvedValue(ok(snapshot(false, 7)))
    window.dispatchEvent(new Event('focus'))
    await expect
      .element(screen.getByRole('switch', { name: label }))
      .toHaveAttribute('aria-checked', 'false')
    await expect.element(textarea).toHaveValue('Keep this draft')
    service.get.mockResolvedValue(ok(snapshot(true, 8)))
    resync()
    await expect
      .element(screen.getByRole('switch', { name: label }))
      .toHaveAttribute('aria-checked', 'true')
    await expect.element(textarea).toHaveValue('Keep this draft')
    expect(service.loadPreferences).toHaveBeenCalledTimes(1)
  })

  it('keeps the shared switch dimensions and readable text at a narrow settings width', async () => {
    const screen = await render(
      <div
        className="settings-page"
        style={{
          ...getFrontendCssVariables(),
          position: 'relative',
          display: 'block',
          width: 340,
          padding: 16
        }}
      >
        <PersonalizationSettingsPage />
      </div>
    )
    const toggle = screen.getByRole('switch', { name: label })
    await expect.element(toggle).toBeVisible()
    const box = toggle.element().getBoundingClientRect()
    const row = toggle.element().closest('.settings-list-row')!.getBoundingClientRect()
    expect(box.width).toBe(44)
    expect(box.height).toBe(24)
    expect(box.right).toBeLessThanOrEqual(row.right)
    expect(
      screen.container.querySelector('.personalization-human-interaction')!.scrollWidth
    ).toBeLessThanOrEqual(340)
    await page.screenshot({
      element: screen.container.querySelector<HTMLElement>('.personalization-human-interaction')!,
      path: '__screenshots__/HumanInteractionSettings.browser.test.tsx/narrow-settings.png'
    })
  })
})
