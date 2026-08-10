import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { DeepSeekProviderSettingsEditor } =
  await import('../../features/settings/pages/configuration/providerSettingsEditors')

function changeSelect(select: HTMLSelectElement, value: string) {
  select.value = value
  select.dispatchEvent(new Event('change', { bubbles: true }))
}

describe('DeepSeekProviderSettingsEditor', () => {
  it('keeps edits local until confirm and normalizes disabled Thinking', async () => {
    const onCancel = vi.fn()
    const onConfirm = vi.fn()
    const screen = await render(
      <DeepSeekProviderSettingsEditor
        initialSettings={{ reasoning: { mode: 'enabled', effort: 'max' } }}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    )

    const mode = document.querySelector<HTMLSelectElement>(
      'select[aria-label="configuration.deepSeekSettings.thinkingMode"]'
    )
    const effort = document.querySelector<HTMLSelectElement>(
      'select[aria-label="configuration.deepSeekSettings.reasoningEffort"]'
    )

    // Labels wrap the native controls, so their accessible names are supplied by their text.
    expect(mode).not.toBeNull()
    expect(effort).not.toBeNull()
    expect(Array.from(mode!.options).map((option) => option.value)).toEqual([
      'provider_default',
      'enabled',
      'disabled'
    ])
    expect(Array.from(effort!.options).map((option) => option.value)).toEqual([
      'provider_default',
      'high',
      'max'
    ])
    expect(document.body.textContent).toContain(
      'configuration.deepSeekSettings.defaultThinkingDescription'
    )
    expect(document.querySelector('.provider-settings-dialog__notes')).toBeNull()
    expect(document.querySelectorAll('.provider-settings-dialog__card li')).toHaveLength(0)
    changeSelect(mode!, 'disabled')

    await expect.poll(() => effort!.value).toBe('provider_default')
    expect(effort!.disabled).toBe(true)
    expect(onConfirm).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    expect(onConfirm).toHaveBeenCalledWith({
      reasoning: { mode: 'disabled', effort: 'provider_default' }
    })
    expect(onCancel).not.toHaveBeenCalled()
  })

  it('does not publish a changed draft when cancelled', async () => {
    const onCancel = vi.fn()
    const onConfirm = vi.fn()
    const screen = await render(
      <DeepSeekProviderSettingsEditor
        initialSettings={{ reasoning: { mode: 'provider_default', effort: 'provider_default' } }}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    )
    const mode = document.querySelector<HTMLSelectElement>(
      'select[aria-label="configuration.deepSeekSettings.thinkingMode"]'
    )!
    changeSelect(mode, 'enabled')

    await screen.getByText('configuration.providerSettings.cancel').click()

    expect(onCancel).toHaveBeenCalledOnce()
    expect(onConfirm).not.toHaveBeenCalled()
  })
})
