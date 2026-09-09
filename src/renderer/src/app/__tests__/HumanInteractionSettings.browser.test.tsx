import type { AgentPromptPreferencesChanged, HumanInteractionSettings } from '@mycopilot/protocol'
import type { HostInvocationResult } from '@mycopilot/host-api'
import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import '../../features/settings/SettingsPage.css'

const service = vi.hoisted(() => ({
  preferencesSubscribe: vi.fn(),
  preferencesUnsubscribe: vi.fn(),
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
    agent: { onPromptPreferencesChanged: service.preferencesSubscribe },
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
    contextProfile: 'full',
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
const label = '允许智能体向人类发起提问与协作'
let changed: (settings: HumanInteractionSettings) => void
let resync: () => void
let preferencesChanged: (event: AgentPromptPreferencesChanged) => void

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
  service.preferencesSubscribe.mockImplementation((handler) => {
    preferencesChanged = handler
    return service.preferencesUnsubscribe
  })
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
    contextProfile: 'full',
    workMode: 'coding',
    tone: 'pragmatic',
    detailLevel: 'medium',
    customInstructions: 'Saved instructions',
    updatedAt: 1
  })
})

describe('Human interaction personalization settings', () => {
  it('immediately saves choices without publishing the instruction draft, then saves only instructions', async () => {
    service.savePreferences.mockImplementation(async (value) => ({ ...value, updatedAt: 2 }))
    const screen = await render(<PersonalizationSettingsPage />)
    const toggle = screen.getByRole('switch', { name: '轻量模式', exact: true })
    await expect.element(toggle).toBeEnabled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    const help = screen.getByRole('button', { name: '查看轻量模式说明' })
    await help.click()
    const dialog = screen.getByRole('dialog', { name: '轻量模式' })
    await expect.element(dialog).toBeVisible()
    await expect
      .element(dialog)
      .toHaveTextContent(
        '打开轻量模式后，基础系统提示词、基础工具集合和对应工具说明会被精简。在新一轮次对话生效。'
      )
    await screen.getByRole('button', { name: '知道了' }).click()
    await expect.element(dialog).not.toBeInTheDocument()
    await expect.element(help).toHaveFocus()
    expect(service.savePreferences).not.toHaveBeenCalled()
    await screen.getByRole('textbox').fill('Preserve my instructions')
    for (const contextProfile of ['minimal', 'full']) {
      await toggle.click()
      await expect
        .poll(() => service.savePreferences.mock.calls.at(-1)?.[0])
        .toEqual({
          contextProfile,
          workMode: 'coding',
          tone: 'pragmatic',
          detailLevel: 'medium',
          customInstructions: 'Saved instructions'
        })
      await expect.element(toggle).toBeEnabled()
      await expect.element(screen.getByRole('textbox')).toHaveValue('Preserve my instructions')
    }
    await screen.getByRole('button', { name: /适用于日常工作/ }).click()
    await expect
      .poll(() => service.savePreferences.mock.calls.at(-1)?.[0])
      .toMatchObject({
        contextProfile: 'full',
        workMode: 'general',
        tone: 'pragmatic',
        customInstructions: 'Saved instructions'
      })
    await screen.getByRole('button', { name: '务实', exact: true }).click()
    await screen.getByRole('option', { name: /亲和/ }).click()
    await expect
      .poll(() => service.savePreferences.mock.calls.at(-1)?.[0])
      .toMatchObject({
        contextProfile: 'full',
        workMode: 'general',
        tone: 'friendly',
        customInstructions: 'Saved instructions'
      })
    const save = screen.getByRole('button', { name: '保存', exact: true })
    expect(
      save.element().closest('.personalization-custom-instructions__heading-row')
    ).not.toBeNull()
    await save.click()
    await expect
      .poll(() => service.savePreferences.mock.calls.at(-1)?.[0])
      .toEqual({
        contextProfile: 'full',
        workMode: 'general',
        tone: 'friendly',
        detailLevel: 'medium',
        customInstructions: 'Preserve my instructions'
      })
    await expect.element(save).toBeDisabled()
    expect(service.savePreferences).toHaveBeenCalledTimes(5)
    expect(service.update).not.toHaveBeenCalled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
  })

  it('keeps capability-center mode changes across a delayed initial read and preserves instruction drafts', async () => {
    const pending = deferred<unknown>()
    service.loadPreferences.mockReturnValueOnce(pending.promise)
    const screen = await render(<PersonalizationSettingsPage />)
    preferencesChanged({ contextProfile: 'minimal', updatedAt: 3 })
    pending.resolve({
      contextProfile: 'full',
      workMode: 'coding',
      tone: 'pragmatic',
      detailLevel: 'medium',
      customInstructions: 'Saved instructions',
      updatedAt: 1
    })
    const toggle = screen.getByRole('switch', { name: '轻量模式', exact: true })
    await expect.element(toggle).toBeEnabled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    await screen.getByRole('textbox').fill('Unsaved draft')
    preferencesChanged({ contextProfile: 'full', updatedAt: 4 })
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    await expect.element(screen.getByRole('textbox')).toHaveValue('Unsaved draft')
    expect(service.savePreferences).not.toHaveBeenCalled()
  })

  it('keeps choices and the latest instruction draft after an immediate save fails', async () => {
    const pending = deferred<unknown>()
    service.savePreferences.mockReturnValueOnce(pending.promise)
    const screen = await render(<PersonalizationSettingsPage />)
    const textarea = screen.getByRole('textbox')
    await expect.element(textarea).toHaveValue('Saved instructions')
    await textarea.fill('Draft before switching')
    const toggle = screen.getByRole('switch', { name: '轻量模式', exact: true })
    await toggle.click()
    await expect.element(toggle).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: /适用于日常工作/ })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '务实', exact: true })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '保存', exact: true })).toBeDisabled()
    await textarea.fill('Newer draft while switching')
    pending.reject(new Error('Unable to save'))
    await expect.element(screen.getByRole('alert')).toBeVisible()
    await expect.element(toggle).toBeEnabled()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
    await expect.element(textarea).toHaveValue('Newer draft while switching')
    service.savePreferences.mockImplementation(async (value) => ({ ...value, updatedAt: 2 }))
    await toggle.click()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    expect(service.savePreferences.mock.calls.at(-1)?.[0].customInstructions).toBe(
      'Saved instructions'
    )
    await expect.element(textarea).toHaveValue('Newer draft while switching')
    await expect.element(screen.getByRole('button', { name: '保存', exact: true })).toBeEnabled()
  })

  it('preserves edits typed during an instruction save and uses only committed instructions for choices', async () => {
    const pending = deferred<Record<string, unknown>>()
    service.savePreferences.mockReturnValueOnce(pending.promise)
    const screen = await render(<PersonalizationSettingsPage />)
    const textarea = screen.getByRole('textbox')
    await expect.element(textarea).toHaveValue('Saved instructions')
    await textarea.fill('Submitted instructions')
    const save = screen.getByRole('button', { name: '保存', exact: true })
    await save.click()
    const submitted = service.savePreferences.mock.calls[0][0]
    await textarea.fill('New draft after submission')
    pending.resolve({ ...submitted, updatedAt: 2 })
    await expect.element(save).toBeEnabled()
    await expect.element(textarea).toHaveValue('New draft after submission')
    service.savePreferences.mockImplementation(async (value) => ({ ...value, updatedAt: 3 }))
    const toggle = screen.getByRole('switch', { name: '轻量模式', exact: true })
    await toggle.click()
    await expect.element(toggle).toHaveAttribute('aria-checked', 'true')
    expect(service.savePreferences.mock.calls.at(-1)?.[0].customInstructions).toBe(
      'Submitted instructions'
    )
    await expect.element(textarea).toHaveValue('New draft after submission')
    // Clearing instructions is also an explicit save; whitespace normalization may come from Host.
    await textarea.fill('')
    await save.click()
    await expect.element(save).toBeDisabled()
    expect(service.savePreferences.mock.calls.at(-1)?.[0]).toMatchObject({
      contextProfile: 'minimal',
      customInstructions: ''
    })
  })

  it('does not overwrite saved preferences with defaults when the initial read fails', async () => {
    service.loadPreferences.mockRejectedValueOnce(new Error('Core unavailable'))
    const screen = await render(<PersonalizationSettingsPage />)
    await expect.element(screen.getByRole('alert')).toBeVisible()
    await expect
      .element(screen.getByRole('switch', { name: '轻量模式', exact: true }))
      .toBeDisabled()
    await expect.element(screen.getByRole('button', { name: /适用于日常工作/ })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '务实', exact: true })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '保存', exact: true })).toBeDisabled()
    expect(service.savePreferences).not.toHaveBeenCalled()
  })

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
    const minimalToggle = screen.getByRole('switch', { name: '轻量模式', exact: true })
    await expect.element(minimalToggle).toBeVisible()
    const minimalBox = minimalToggle.element().getBoundingClientRect()
    const toneBox = screen.container
      .querySelector('.personalization-tone-section')!
      .getBoundingClientRect()
    const minimalRow = screen.container
      .querySelector('.personalization-minimal-mode')!
      .getBoundingClientRect()
    expect(minimalRow.top).toBeGreaterThan(toneBox.bottom)
    expect(minimalBox.width).toBe(44)
    expect(minimalBox.height).toBe(24)
    expect(
      screen.container.querySelector('.personalization-minimal-mode')!.scrollWidth
    ).toBeLessThanOrEqual(340)
    await page.screenshot({
      element: screen.container.querySelector<HTMLElement>('.personalization-minimal-mode')!,
      path: '__screenshots__/HumanInteractionSettings.browser.test.tsx/lightweight-mode.png'
    })
    const customHeading = screen.container.querySelector<HTMLElement>(
      '.personalization-custom-instructions__heading-row'
    )!
    const save = screen.getByRole('button', { name: '保存', exact: true })
    const titleBox = customHeading.querySelector('h2')!.getBoundingClientRect()
    const saveBox = save.element().getBoundingClientRect()
    expect(saveBox.left).toBeGreaterThan(titleBox.right)
    expect(
      Math.abs(saveBox.top + saveBox.height / 2 - (titleBox.top + titleBox.height / 2))
    ).toBeLessThan(1)
    await page.screenshot({
      element: customHeading,
      path: '__screenshots__/HumanInteractionSettings.browser.test.tsx/custom-instructions-save.png'
    })
  })
})
