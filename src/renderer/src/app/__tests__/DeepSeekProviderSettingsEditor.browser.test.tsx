import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { DeepSeekProviderSettingsEditor } =
  await import('../../features/settings/pages/configuration/providerSettingsEditors')

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

    const effort = screen.getByRole('button', {
      name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortMax'
    })

    await effort.click()
    expect(
      Array.from(
        document.querySelectorAll<HTMLElement>(
          '.provider-settings-dialog__select .settings-select__option-label'
        )
      ).map((option) => option.textContent)
    ).toEqual([
      'configuration.deepSeekSettings.effortProviderDefault',
      'configuration.deepSeekSettings.effortHigh',
      'configuration.deepSeekSettings.effortMax'
    ])
    await screen.getByRole('option', { name: 'configuration.deepSeekSettings.effortMax' }).click()

    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingEnabled'
      })
      .click()
    expect(
      Array.from(
        document.querySelectorAll<HTMLElement>(
          '.provider-settings-dialog__select .settings-select__option-label'
        )
      ).map((option) => option.textContent)
    ).toEqual([
      'configuration.deepSeekSettings.thinkingProviderDefault',
      'configuration.deepSeekSettings.thinkingEnabled',
      'configuration.deepSeekSettings.thinkingDisabled'
    ])
    expect(document.body.textContent).toContain(
      'configuration.deepSeekSettings.defaultThinkingDescription'
    )
    expect(document.querySelector('.provider-settings-dialog__notes')).toBeNull()
    expect(document.querySelectorAll('.provider-settings-dialog__card li')).toHaveLength(0)
    await screen
      .getByRole('option', { name: 'configuration.deepSeekSettings.thinkingDisabled' })
      .click()

    const disabledEffort = document.querySelector<HTMLButtonElement>(
      'button[aria-label="configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortProviderDefault"]'
    )
    await expect.poll(() => disabledEffort?.disabled).toBe(true)
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
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingProviderDefault'
      })
      .click()
    await screen
      .getByRole('option', { name: 'configuration.deepSeekSettings.thinkingEnabled' })
      .click()

    await screen.getByText('configuration.providerSettings.cancel').click()

    expect(onCancel).toHaveBeenCalledOnce()
    expect(onConfirm).not.toHaveBeenCalled()
  })

  it('closes an open settings menu before closing the dialog with Escape', async () => {
    const onCancel = vi.fn()
    const screen = await render(
      <DeepSeekProviderSettingsEditor
        initialSettings={{ reasoning: { mode: 'enabled', effort: 'high' } }}
        onCancel={onCancel}
        onConfirm={vi.fn()}
      />
    )

    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingEnabled'
      })
      .click()
    expect(
      document.querySelector('.provider-settings-dialog__select[data-open="true"]')
    ).not.toBeNull()

    document.activeElement?.dispatchEvent(
      new KeyboardEvent('keydown', { bubbles: true, key: 'Escape' })
    )

    await expect
      .poll(() => document.querySelector('.provider-settings-dialog__select[data-open="true"]'))
      .toBeNull()
    expect(onCancel).not.toHaveBeenCalled()

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))
    expect(onCancel).toHaveBeenCalledOnce()
  })
})
