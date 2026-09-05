import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelConfig } from '../../config/modelConfig'
import type { ProviderVendorModelPolicyDescriptor } from '@mycopilot/protocol'
import {
  SettingsSearchNavigationProvider,
  type SettingsNavigationTarget
} from '../../features/settings/settingsSearchNavigation'

const service = vi.hoisted(() => ({
  deleteModel: vi.fn(),
  resolveProviderVendorModelPolicy: vi.fn(),
  setApiUrl: vi.fn(),
  setSearchMode: vi.fn(),
  toggleModel: vi.fn(),
  updateApiToken: vi.fn(),
  updateTavilyApiKey: vi.fn(),
  upsertModel: vi.fn()
}))

const model: ModelConfig = {
  id: 'model-settings-search',
  providerModelId: 'test-model',
  displayName: 'Existing model',
  contextWindowTokens: 128_000,
  apiTokenOverrideStatus: 'missing',
  apiTokenOverrideMutation: { type: 'keep' },
  supportsImage: false,
  inputPrice: '0',
  cachedInputPrice: '',
  outputPrice: '0',
  enabled: true,
  providerProfileConfig: {
    schemaVersion: 1,
    profile: { id: 'generic_openai_chat', version: 1 },
    reasoning: { mode: 'provider_default', effort: 'provider_default' }
  },
  providerProfileUpdate: { kind: 'select_generic' }
}

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    ...service,
    apiUrl: 'https://provider.example/v1/chat/completions',
    apiTokenStatus: 'configured',
    models: [model],
    providerProfileDescriptors: [
      {
        profileId: 'generic_openai_chat',
        profileVersion: 1,
        displayName: 'Generic OpenAI Chat',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'none',
        selectable: true
      }
    ],
    providerVendorDescriptors: [{ vendorId: 'generic', displayName: 'Generic', selectable: true }],
    searchMode: 'auto',
    tavilyApiKeyStatus: 'configured'
  })
}))

vi.mock('../../features/settings/pages/configuration/ImageGenerationSettings', () => ({
  ImageGenerationSettings: () => null
}))

const { ConfigurationSettingsPage } =
  await import('../../features/settings/pages/ConfigurationSettingsPage')
const { ModelForm } = await import('../../features/settings/pages/configuration/ModelForm')

function renderPage(target: SettingsNavigationTarget | null) {
  return (
    <SettingsSearchNavigationProvider target={target}>
      <ConfigurationSettingsPage onNavigateSettingsRoot={vi.fn()} />
    </SettingsSearchNavigationProvider>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  service.resolveProviderVendorModelPolicy.mockResolvedValue({
    status: 'supported',
    vendorId: 'generic',
    modelFamily: 'generic_openai_chat',
    settingsKind: 'none',
    imageInput: 'user_configurable',
    settings: { kind: 'generic', defaultSettings: { kind: 'generic' } }
  } satisfies ProviderVendorModelPolicyDescriptor)
  service.upsertModel.mockResolvedValue(undefined)
})

describe('configuration setting search navigation', () => {
  it('cancels an unresolved provider-dialog navigation when the search target is cleared', async () => {
    let resolvePolicy!: (value: ProviderVendorModelPolicyDescriptor) => void
    const pendingPolicy = new Promise<ProviderVendorModelPolicyDescriptor>((resolve) => {
      resolvePolicy = resolve
    })
    const resolveProviderVendorModelPolicy = vi.fn(() => pendingPolicy)
    const configuredModel: ModelConfig = {
      ...model,
      providerProfileConfig: {
        schemaVersion: 1,
        profile: { id: 'deepseek_v4_chat', version: 1 },
        reasoning: { mode: 'enabled', effort: 'high' }
      }
    }
    const form = (target: SettingsNavigationTarget | null) => (
      <SettingsSearchNavigationProvider target={target}>
        <ModelForm
          model={configuredModel}
          globalApiUrl="https://provider.example/v1/chat/completions"
          providerProfileDescriptors={[
            {
              profileId: 'deepseek_v4_chat',
              profileVersion: 1,
              displayName: 'DeepSeek',
              compatibleDialects: ['openai_chat_completions'],
              settingsKind: 'deepseek_v4_chat',
              selectable: true
            }
          ]}
          providerVendorDescriptors={[
            { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true }
          ]}
          resolveProviderVendorModelPolicy={resolveProviderVendorModelPolicy}
          onCancel={vi.fn()}
          onSave={service.upsertModel}
        />
      </SettingsSearchNavigationProvider>
    )
    const screen = await render(
      form({
        page: 'configuration',
        id: 'configuration.model.deepseek.reasoningEffort',
        view: 'model-deepseek',
        revision: 1
      })
    )
    await expect.poll(() => resolveProviderVendorModelPolicy.mock.calls.length).toBe(1)
    await screen.rerender(form(null))
    resolvePolicy({
      status: 'supported',
      vendorId: 'deepseek',
      modelFamily: 'deepseek_v4_chat',
      settingsKind: 'deepseek',
      imageInput: 'unsupported',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoningModes: ['provider_default', 'enabled', 'disabled'],
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: {
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }
      }
    })
    await expect
      .element(screen.getByRole('button', { name: 'configuration.providerSettings.open' }))
      .toBeEnabled()
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    expect(service.upsertModel).not.toHaveBeenCalled()
  })

  it('requires an explicit model choice and continues to the requested advanced setting', async () => {
    const screen = await render(
      renderPage({
        page: 'configuration',
        id: 'configuration.model.apiToken',
        view: 'model-advanced',
        prerequisiteId: 'configuration.models',
        revision: 1
      })
    )

    await expect
      .element(screen.getByRole('heading', { name: 'configuration.modelManager' }))
      .toBeVisible()
    await expect
      .element(screen.getByPlaceholder('configuration.displayNamePlaceholder'))
      .not.toBeInTheDocument()
    expect(service.resolveProviderVendorModelPolicy).not.toHaveBeenCalled()
    expect(document.querySelector('[data-setting-id="configuration.models"]')).not.toBeNull()

    await screen.getByRole('button', { name: 'configuration.edit: Existing model' }).click()
    await expect
      .element(screen.getByRole('button', { name: 'configuration.more' }))
      .toHaveAttribute('aria-expanded', 'true')
    await expect
      .element(screen.getByPlaceholder('configuration.displayNamePlaceholder'))
      .toHaveValue('Existing model')
    expect(
      document.querySelector('[data-setting-id="configuration.model.apiToken"]')
    ).not.toBeNull()
    expect(service.upsertModel).not.toHaveBeenCalled()
    expect(service.updateApiToken).not.toHaveBeenCalled()
    expect(service.toggleModel).not.toHaveBeenCalled()
  })

  it('preserves the current model draft while navigating between its fields and saves only on submit', async () => {
    const screen = await render(
      renderPage({
        page: 'configuration',
        id: 'configuration.models',
        view: 'model',
        revision: 1
      })
    )
    await screen.getByRole('button', { name: 'configuration.edit: Existing model' }).click()
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('My unsaved name')

    const target: SettingsNavigationTarget = {
      page: 'configuration',
      id: 'configuration.model.apiUrl',
      view: 'model-advanced',
      revision: 2
    }
    await screen.rerender(renderPage(target))
    await expect
      .element(screen.getByRole('button', { name: 'configuration.more' }))
      .toHaveAttribute('aria-expanded', 'true')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen.rerender(renderPage({ ...target, revision: 3 }))
    await expect
      .element(screen.getByRole('button', { name: 'configuration.more' }))
      .toHaveAttribute('aria-expanded', 'true')
    await expect
      .element(screen.getByPlaceholder('configuration.displayNamePlaceholder'))
      .toHaveValue('My unsaved name')
    expect(service.upsertModel).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'configuration.save', exact: true }).click()
    await expect.poll(() => service.upsertModel.mock.calls.length).toBe(1)
    expect(service.upsertModel.mock.calls[0]?.[0]).toMatchObject({
      id: model.id,
      displayName: 'My unsaved name',
      apiTokenOverrideMutation: { type: 'keep' }
    })
    await expect
      .element(screen.getByRole('heading', { name: 'configuration.modelManager' }))
      .toBeVisible()

    await screen.rerender(
      renderPage({ page: 'configuration', id: 'configuration.apiUrl', revision: 4 })
    )
    await expect
      .element(screen.getByRole('textbox', { name: 'API URL', exact: true }))
      .toBeVisible()
    expect(service.setApiUrl).not.toHaveBeenCalled()
    expect(service.updateApiToken).not.toHaveBeenCalled()
    expect(service.updateTavilyApiKey).not.toHaveBeenCalled()
  })
})
