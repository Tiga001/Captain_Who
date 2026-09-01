import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { ProviderFamilySettingsDescriptor } from '@mycopilot/protocol'
import '../../features/settings/pages/ConfigurationSettingsPage.css'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { DeepSeekProviderSettingsEditor, MoonshotProviderSettingsEditor } =
  await import('../../features/settings/pages/configuration/providerSettingsEditors')

const deepSeekDescriptor: Extract<ProviderFamilySettingsDescriptor, { kind: 'deepseek_v4_chat' }> =
  {
    kind: 'deepseek_v4_chat',
    reasoningModes: ['provider_default', 'enabled', 'disabled'],
    reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
    defaultSettings: {
      kind: 'deepseek_v4_chat',
      reasoning: { mode: 'provider_default', effort: 'provider_default' }
    }
  }

const moonshotK3Descriptor: Extract<
  ProviderFamilySettingsDescriptor,
  { kind: 'moonshot_k3_chat' }
> = {
  kind: 'moonshot_k3_chat',
  reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
  defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
}

const moonshotK27Descriptor: Extract<
  ProviderFamilySettingsDescriptor,
  { kind: 'moonshot_k2_7_code_chat' }
> = {
  kind: 'moonshot_k2_7_code_chat',
  defaultSettings: { kind: 'moonshot_k2_7_code_chat' }
}

const moonshotK26Descriptor: Extract<
  ProviderFamilySettingsDescriptor,
  { kind: 'moonshot_k2_6_chat' }
> = {
  kind: 'moonshot_k2_6_chat',
  thinkingModes: ['provider_default', 'enabled', 'disabled', 'enabled_keep_all'],
  defaultSettings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }
}

function DeepSeekDialogHarness({ onCancel }: { onCancel: () => void }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open DeepSeek settings
      </button>
      {open ? (
        <DeepSeekProviderSettingsEditor
          descriptor={deepSeekDescriptor}
          initialSettings={{
            kind: 'deepseek_v4_chat',
            reasoning: { mode: 'enabled', effort: 'high' }
          }}
          onCancel={() => {
            onCancel()
            setOpen(false)
          }}
          onConfirm={vi.fn()}
        />
      ) : null}
    </>
  )
}

describe('DeepSeekProviderSettingsEditor', () => {
  it('keeps edits local until confirm and normalizes disabled Thinking', async () => {
    const onCancel = vi.fn()
    const onConfirm = vi.fn()
    const screen = await render(
      <DeepSeekProviderSettingsEditor
        descriptor={deepSeekDescriptor}
        initialSettings={{
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'enabled', effort: 'max' }
        }}
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
      'configuration.deepSeekSettings.effortLow',
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
      kind: 'deepseek_v4_chat',
      reasoning: { mode: 'disabled', effort: 'provider_default' }
    })
    expect(onCancel).not.toHaveBeenCalled()
  })

  it('does not publish a changed draft when cancelled', async () => {
    const onCancel = vi.fn()
    const onConfirm = vi.fn()
    const screen = await render(
      <DeepSeekProviderSettingsEditor
        descriptor={deepSeekDescriptor}
        initialSettings={{
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }}
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
        descriptor={deepSeekDescriptor}
        initialSettings={{
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'enabled', effort: 'high' }
        }}
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

  it('moves focus into the shared shell, traps Tab, and restores the opener', async () => {
    const onCancel = vi.fn()
    const screen = await render(<DeepSeekDialogHarness onCancel={onCancel} />)
    const opener = screen.getByRole('button', { name: 'Open DeepSeek settings' })

    await opener.click()
    const card = document.querySelector<HTMLElement>('.provider-settings-dialog__card')!
    const close = card.querySelector<HTMLButtonElement>('.provider-settings-dialog__close')!
    const cancel = card.querySelector<HTMLButtonElement>('.secondary-settings-button')!
    const confirm = card.querySelector<HTMLButtonElement>('.primary-settings-button')!
    await expect.poll(() => document.activeElement).toBe(cancel)

    close.focus()
    await userEvent.keyboard('{Shift>}{Tab}{/Shift}')
    await expect.poll(() => document.activeElement).toBe(confirm)

    await userEvent.keyboard('{Tab}')
    await expect.poll(() => document.activeElement).toBe(close)

    await userEvent.keyboard('{Escape}')
    expect(onCancel).toHaveBeenCalledOnce()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(opener).toHaveFocus()

    await opener.click()
    const backdrop = document.querySelector<HTMLElement>('.provider-settings-dialog__backdrop')!
    backdrop.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    await expect.poll(() => onCancel.mock.calls.length).toBe(2)
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(opener).toHaveFocus()
  })
})

describe('MoonshotProviderSettingsEditor', () => {
  it('shows only the Host-declared K3 effort matrix and preserved-thinking notice', async () => {
    const onConfirm = vi.fn()
    const screen = await render(
      <MoonshotProviderSettingsEditor
        descriptor={moonshotK3Descriptor}
        initialSettings={{ kind: 'moonshot_k3_chat', reasoningEffort: 'low' }}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    )

    expect(document.body.textContent).toContain('configuration.moonshotSettings.k3Description')
    expect(document.body.textContent).toContain(
      'configuration.moonshotSettings.alwaysPreservedThinking'
    )
    expect(document.body.textContent).not.toContain('configuration.moonshotSettings.thinkingMode')
    await screen
      .getByRole('button', {
        name: 'configuration.moonshotSettings.reasoningEffort: configuration.moonshotSettings.effortLow'
      })
      .click()
    expect(
      Array.from(
        document.querySelectorAll<HTMLElement>(
          '.provider-settings-dialog__select .settings-select__option-label'
        )
      ).map((option) => option.textContent)
    ).toEqual([
      'configuration.moonshotSettings.effortProviderDefault',
      'configuration.moonshotSettings.effortLow',
      'configuration.moonshotSettings.effortHigh',
      'configuration.moonshotSettings.effortMax'
    ])
    await screen.getByRole('option', { name: 'configuration.moonshotSettings.effortHigh' }).click()
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()

    expect(onConfirm).toHaveBeenCalledWith({
      kind: 'moonshot_k3_chat',
      reasoningEffort: 'high'
    })
  })

  it('keeps K2.7 read-only with no reasoning effort or thinking control', async () => {
    const onConfirm = vi.fn()
    const screen = await render(
      <MoonshotProviderSettingsEditor
        descriptor={moonshotK27Descriptor}
        initialSettings={{ kind: 'moonshot_k2_7_code_chat' }}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    )

    expect(document.body.textContent).toContain('configuration.moonshotSettings.k27Description')
    expect(document.body.textContent).toContain(
      'configuration.moonshotSettings.alwaysPreservedThinking'
    )
    expect(document.body.textContent).not.toContain(
      'configuration.moonshotSettings.reasoningEffort'
    )
    expect(document.body.textContent).not.toContain('configuration.moonshotSettings.thinkingMode')
    expect(document.querySelector('.provider-settings-dialog__select')).toBeNull()

    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    expect(onConfirm).toHaveBeenCalledWith({ kind: 'moonshot_k2_7_code_chat' })
  })

  it('shows all and only the legal K2.6 thinking modes', async () => {
    const onConfirm = vi.fn()
    const screen = await render(
      <MoonshotProviderSettingsEditor
        descriptor={moonshotK26Descriptor}
        initialSettings={{ kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }}
        onCancel={vi.fn()}
        onConfirm={onConfirm}
      />
    )

    expect(document.body.textContent).toContain('configuration.moonshotSettings.k26Description')
    expect(document.body.textContent).not.toContain(
      'configuration.moonshotSettings.reasoningEffort'
    )
    expect(document.body.textContent).not.toContain(
      'configuration.moonshotSettings.alwaysPreservedThinking'
    )
    await screen
      .getByRole('button', {
        name: 'configuration.moonshotSettings.thinkingMode: configuration.moonshotSettings.thinkingProviderDefault'
      })
      .click()
    expect(
      Array.from(
        document.querySelectorAll<HTMLElement>(
          '.provider-settings-dialog__select .settings-select__option-label'
        )
      ).map((option) => option.textContent)
    ).toEqual([
      'configuration.moonshotSettings.thinkingProviderDefault',
      'configuration.moonshotSettings.thinkingEnabled',
      'configuration.moonshotSettings.thinkingDisabled',
      'configuration.moonshotSettings.thinkingEnabledKeepAll'
    ])
    await screen
      .getByRole('option', {
        name: 'configuration.moonshotSettings.thinkingEnabledKeepAll'
      })
      .click()
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()

    expect(onConfirm).toHaveBeenCalledWith({
      kind: 'moonshot_k2_6_chat',
      thinkingMode: 'enabled_keep_all'
    })
  })
})

describe('ProviderSettingsDialogShell parity', () => {
  it('gives DeepSeek and Moonshot the same structure and computed shell geometry', async () => {
    await render(
      <>
        <DeepSeekProviderSettingsEditor
          descriptor={deepSeekDescriptor}
          initialSettings={deepSeekDescriptor.defaultSettings}
          onCancel={vi.fn()}
          onConfirm={vi.fn()}
        />
        <MoonshotProviderSettingsEditor
          descriptor={moonshotK3Descriptor}
          initialSettings={moonshotK3Descriptor.defaultSettings}
          onCancel={vi.fn()}
          onConfirm={vi.fn()}
        />
      </>
    )

    const cards = Array.from(
      document.querySelectorAll<HTMLElement>('.provider-settings-dialog__card')
    )
    const backdrops = Array.from(
      document.querySelectorAll<HTMLElement>('.provider-settings-dialog__backdrop')
    )
    expect(cards).toHaveLength(2)
    expect(backdrops).toHaveLength(2)

    const structure = (card: HTMLElement) =>
      Array.from(card.children).map((child) => ({
        className: child.className,
        tagName: child.tagName
      }))
    expect(structure(cards[0]!)).toEqual(structure(cards[1]!))
    expect(structure(cards[0]!)).toEqual([
      { className: 'provider-settings-dialog__close', tagName: 'BUTTON' },
      { className: '', tagName: 'H2' },
      { className: 'provider-settings-dialog__intro', tagName: 'P' },
      { className: 'provider-settings-dialog__fields', tagName: 'DIV' },
      { className: 'provider-settings-dialog__actions', tagName: 'DIV' }
    ])

    const geometry = (element: HTMLElement) => {
      const style = window.getComputedStyle(element)
      return {
        display: style.display,
        gap: style.gap,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        paddingRight: style.paddingRight,
        paddingTop: style.paddingTop,
        position: style.position,
        width: style.width
      }
    }
    expect(geometry(cards[0]!)).toEqual(geometry(cards[1]!))
    expect(geometry(cards[0]!)).toMatchObject({
      display: 'grid',
      gap: '14px',
      paddingBottom: '24px',
      paddingLeft: '28px',
      paddingRight: '28px',
      paddingTop: '26px',
      position: 'relative',
      width: `${Math.min(500, window.innerWidth - 48)}px`
    })
    expect(geometry(backdrops[0]!)).toEqual(geometry(backdrops[1]!))
    expect(window.getComputedStyle(backdrops[0]!).position).toBe('fixed')
  })
})
