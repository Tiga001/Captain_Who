import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ProviderProfileUiDescriptor } from '@mycopilot/protocol'
import type { ModelConfig } from '../../config/modelConfig'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { ModelForm } = await import('../../features/settings/pages/configuration/ModelForm')

const descriptors: ProviderProfileUiDescriptor[] = [
  {
    profileId: 'generic_openai_chat',
    profileVersion: 1,
    displayName: 'Generic OpenAI Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: true
  },
  {
    profileId: 'deepseek_v4_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4 Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'deepseek_v4_chat',
    selectable: true
  }
]

const model: ModelConfig = {
  id: 'provider-model',
  displayName: 'Provider Model',
  supportsImage: false,
  contextWindowTokens: 128_000,
  providerProfileConfig: {
    schemaVersion: 1,
    profile: { id: 'generic_openai_chat', version: 1 },
    reasoning: { mode: 'provider_default', effort: 'provider_default' }
  },
  inputPrice: '0',
  outputPrice: '0',
  enabled: true
}

describe('ModelForm Provider Profile controls', () => {
  it('builds the registered choices only from selectable Host descriptors', async () => {
    const screen = await render(
      <ModelForm
        model={model}
        providerProfileDescriptors={descriptors.map((descriptor) =>
          descriptor.profileId === 'deepseek_v4_chat'
            ? { ...descriptor, selectable: false }
            : descriptor
        )}
        onCancel={vi.fn()}
        onSave={vi.fn()}
      />
    )

    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    expect(
      Array.from(
        document.querySelectorAll<HTMLElement>(
          '.model-provider-profile-select .settings-select__option-label'
        )
      ).map((option) => option.textContent)
    ).toEqual(['configuration.providerProfile.generic'])
  })

  it('keeps an unsupported profile visible as a disabled current option', async () => {
    const screen = await render(
      <ModelForm
        model={{
          ...model,
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'future_vendor_chat', version: 7 },
            reasoning: { mode: 'provider_default', effort: 'provider_default' }
          }
        }}
        providerProfileDescriptors={descriptors}
        onCancel={vi.fn()}
        onSave={vi.fn()}
      />
    )

    const triggerName =
      'configuration.providerProfile.vendor: configuration.providerProfile.unsupported (future_vendor_chat@7)'
    await screen.getByRole('button', { name: triggerName }).click()
    const unsupported = document.querySelector<HTMLButtonElement>(
      '.model-provider-profile-select .settings-select__option:disabled'
    )

    expect(unsupported?.textContent).toContain('future_vendor_chat@7')
    expect(unsupported?.disabled).toBe(true)
    await expect
      .poll(() => document.activeElement?.textContent)
      .toContain('configuration.providerProfile.generic')
    await expect
      .element(screen.getByRole('option', { name: 'configuration.providerProfile.generic' }))
      .toBeVisible()
  })

  it('offers an explicit Generic rematch when the stored Generic dialect may be stale', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        model={model}
        providerProfileDescriptors={descriptors}
        onCancel={vi.fn()}
        onSave={onSave}
      />
    )

    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.generic' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({
      kind: 'select_generic'
    })
  })

  it('atomically rematches Generic when a model-level API URL is explicitly changed', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        model={model}
        providerProfileDescriptors={descriptors}
        onCancel={vi.fn()}
        onSave={onSave}
      />
    )

    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByPlaceholder('configuration.modelApiUrlPlaceholder')
      .fill('https://api.anthropic.com/v1/messages')
    await screen
      .getByPlaceholder('configuration.modelApiTokenPlaceholder')
      .fill('replacement-token')
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toEqual(
      expect.objectContaining({
        apiUrlOverride: 'https://api.anthropic.com/v1/messages',
        providerProfileUpdate: { kind: 'select_generic' }
      })
    )
  })

  it('submits an explicit DeepSeek selection with public settings only', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        model={model}
        providerProfileDescriptors={descriptors}
        onCancel={vi.fn()}
        onSave={onSave}
      />
    )

    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'DeepSeek V4 Chat' }).click()

    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingProviderDefault'
      })
      .click()
    await screen
      .getByRole('option', { name: 'configuration.deepSeekSettings.thinkingEnabled' })
      .click()
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortProviderDefault'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.deepSeekSettings.effortHigh' }).click()
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    const submitted = onSave.mock.calls[0]![0]
    expect(submitted.providerProfileUpdate).toEqual({
      kind: 'select_registered_profile',
      profileId: 'deepseek_v4_chat',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoning: { mode: 'enabled', effort: 'high' }
      }
    })
    expect(JSON.stringify(submitted.providerProfileUpdate)).not.toMatch(
      /profileVersion|revision|capabilit/i
    )
  })

  it('stays open and presents only a stable local error when the Host save fails', async () => {
    const onSave = vi.fn().mockRejectedValue(new Error('raw provider payload'))
    const screen = await render(
      <ModelForm
        model={model}
        providerProfileDescriptors={descriptors}
        onCancel={vi.fn()}
        onSave={onSave}
      />
    )

    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Rejected draft')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'DeepSeek V4 Chat' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('configuration.saveFailedSafe')
    await expect
      .element(screen.getByRole('heading', { name: 'configuration.editModel' }))
      .toBeVisible()
    await expect
      .element(screen.getByPlaceholder('configuration.displayNamePlaceholder'))
      .toHaveValue('Provider Model')
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
        })
      )
      .toBeVisible()
    expect(document.body.textContent).not.toContain('raw provider payload')
  })
})
