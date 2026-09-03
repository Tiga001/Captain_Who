import { StrictMode, useState } from 'react'
import { HostInvocationError } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelSettingsSnapshot } from '../../features/storage/storageClient'

const service = vi.hoisted(() => ({
  loadModelSettings: vi.fn(),
  loadProviderProfileUiDescriptors: vi.fn(),
  loadProviderVendorDescriptors: vi.fn(),
  resolveProviderVendorModelPolicy: vi.fn(),
  saveModelSettings: vi.fn(),
  showToast: vi.fn()
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadModelSettings: service.loadModelSettings,
  loadProviderProfileUiDescriptors: service.loadProviderProfileUiDescriptors,
  loadProviderVendorDescriptors: service.loadProviderVendorDescriptors,
  resolveProviderVendorModelPolicy: service.resolveProviderVendorModelPolicy,
  saveModelSettings: service.saveModelSettings
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => key
  })
}))

const { ModelSettingsProvider, useModelSettings } =
  await import('../../config/ModelSettingsProvider')

const storedSettings: ModelSettingsSnapshot = {
  configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
  apiUrl: 'https://provider.example/v1/chat/completions',
  apiTokenStatus: 'configured',
  searchMode: 'auto',
  tavilyApiKeyStatus: 'missing',
  models: [
    {
      id: 'stored-model',
      providerModelId: 'provider-stored-model',
      displayName: 'Stored Model',
      apiTokenOverrideStatus: 'missing',
      apiTokenOverrideMutation: { type: 'keep' },
      supportsImage: false,
      inputPrice: '0',
      cachedInputPrice: '',
      outputPrice: '0',
      providerProfileConfig: {
        schemaVersion: 1,
        profile: { id: 'generic_openai_chat', version: 1 },
        reasoning: { mode: 'provider_default', effort: 'provider_default' }
      },
      providerProfileUpdate: { kind: 'unchanged' },
      enabled: true
    }
  ]
}

function ModelSettingsProbe() {
  const {
    apiUrl,
    enabledModels,
    providerProfileDescriptors,
    providerVendorDescriptors,
    resolveProviderVendorModelPolicy,
    setApiUrl
  } = useModelSettings()

  return (
    <div>
      <span data-testid="api-url">{apiUrl}</span>
      <span data-testid="enabled-models">{enabledModels.map((model) => model.id).join(',')}</span>
      <span data-testid="provider-profiles">
        {providerProfileDescriptors.map((profile) => profile.profileId).join(',')}
      </span>
      <span data-testid="provider-vendors">
        {providerVendorDescriptors.map((vendor) => vendor.vendorId).join(',')}
      </span>
      <button
        type="button"
        onClick={() => {
          void resolveProviderVendorModelPolicy({
            vendorId: 'moonshot',
            modelId: 'kimi-k3',
            dialect: 'openai_chat_completions'
          })
        }}
      >
        resolve vendor
      </button>
      <button
        type="button"
        onClick={() => {
          void setApiUrl('https://api.anthropic.com/v1/messages').catch(() => undefined)
        }}
      >
        change URL
      </button>
    </div>
  )
}

function ModelRenameProbe() {
  const { models, upsertModel } = useModelSettings()

  return (
    <div>
      <span data-testid="model-ids">{models.map((model) => model.id).join(',')}</span>
      <button
        type="button"
        onClick={() => {
          const source = models.find((model) => model.id === 'model-a')
          if (source) {
            void upsertModel({ ...source, displayName: 'Model B' }).catch(() => undefined)
          }
        }}
      >
        rename model
      </button>
    </div>
  )
}

function ModelCreateProbe() {
  const { models, upsertModel } = useModelSettings()
  const [savedId, setSavedId] = useState('')

  return (
    <div>
      <span data-testid="created-model-ids">{models.map((model) => model.id).join(',')}</span>
      <span data-testid="created-model-display-names">
        {models.map((model) => model.displayName).join(',')}
      </span>
      <span data-testid="created-model-result">{savedId}</span>
      <button
        type="button"
        onClick={() => {
          void upsertModel({
            id: null,
            providerModelId: 'provider-stored-model',
            displayName: 'New Model',
            apiTokenOverrideStatus: 'missing',
            apiTokenOverrideMutation: { type: 'keep' },
            supportsImage: false,
            inputPrice: '0',
            cachedInputPrice: '',
            outputPrice: '0',
            providerProfileUpdate: { kind: 'select_generic' },
            enabled: true
          })
            .then((savedModel) => setSavedId(savedModel.id))
            .catch((error: unknown) =>
              setSavedId(`error:${error instanceof Error ? error.message : 'unknown'}`)
            )
        }}
      >
        create model
      </button>
    </div>
  )
}

function CredentialMutationProbe() {
  const { apiTokenStatus, searchMode, tavilyApiKeyStatus, updateApiToken, updateTavilyApiKey } =
    useModelSettings()

  return (
    <div>
      <span data-testid="api-token-status">{apiTokenStatus}</span>
      <span data-testid="tavily-status">{tavilyApiKeyStatus}</span>
      <span data-testid="search-mode">{searchMode}</span>
      <button type="button" onClick={() => void updateApiToken({ type: 'clear' }).catch(() => {})}>
        clear API token
      </button>
      <button
        type="button"
        onClick={() => void updateTavilyApiKey({ type: 'clear' }).catch(() => {})}
      >
        clear Tavily key
      </button>
    </div>
  )
}

function ModelOverrideCredentialProbe() {
  const { enabledModels, models, upsertModel } = useModelSettings()
  const target = models.find((model) => model.id === 'override-model')

  return (
    <div>
      <span data-testid="override-status">{target?.apiTokenOverrideStatus}</span>
      <span data-testid="override-url">{target?.apiUrlOverride}</span>
      <span data-testid="override-enabled-models">
        {enabledModels.map((model) => model.id).join(',')}
      </span>
      <button
        type="button"
        disabled={!target}
        onClick={() => {
          if (target) {
            void upsertModel({
              ...target,
              apiTokenOverrideMutation: { type: 'clear' }
            }).catch(() => {})
          }
        }}
      >
        clear model override
      </button>
    </div>
  )
}

beforeEach(() => {
  service.loadModelSettings.mockReset()
  service.loadProviderProfileUiDescriptors.mockReset().mockResolvedValue([])
  service.loadProviderVendorDescriptors.mockReset().mockResolvedValue([
    { vendorId: 'generic', displayName: 'Generic', selectable: true },
    { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
    { vendorId: 'moonshot', displayName: 'Moonshot', selectable: true }
  ])
  service.resolveProviderVendorModelPolicy.mockReset().mockResolvedValue({
    status: 'supported',
    vendorId: 'moonshot',
    modelFamily: 'moonshot_k3_chat',
    settingsKind: 'moonshot',
    imageInput: 'supported',
    settings: {
      kind: 'moonshot_k3_chat',
      reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
      defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
    }
  })
  service.saveModelSettings.mockReset().mockImplementation(async (settings) => ({
    ...settings,
    configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000002'
  }))
  service.showToast.mockReset()
})

describe('ModelSettingsProvider hydration', () => {
  it('loads the durable snapshot under StrictMode without writing it back', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)

    const screen = await render(
      <StrictMode>
        <ModelSettingsProvider>
          <ModelSettingsProbe />
        </ModelSettingsProvider>
      </StrictMode>
    )

    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)
    await expect.element(screen.getByTestId('enabled-models')).toHaveTextContent('stored-model')
    expect(service.saveModelSettings).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'change URL' }).click()
    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings).toHaveBeenLastCalledWith(
      {
        apiUrl: 'https://api.anthropic.com/v1/messages',
        apiTokenMutation: { type: 'keep' },
        searchMode: storedSettings.searchMode,
        tavilyApiKeyMutation: { type: 'keep' },
        models: [
          {
            ...storedSettings.models[0],
            providerProfileUpdate: { kind: 'unchanged' }
          }
        ]
      },
      storedSettings.configurationRevision
    )
  })

  it('atomically rematches only inherited known Generic models when the global URL changes', async () => {
    const settings: ModelSettingsSnapshot = {
      ...storedSettings,
      models: [
        {
          ...storedSettings.models[0]!,
          id: 'generic-inherited',
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'generic_openai_chat', version: 1 },
            reasoning: { mode: 'provider_default', effort: 'provider_default' }
          }
        },
        {
          ...storedSettings.models[0]!,
          id: 'generic-override',
          apiUrlOverride: 'https://override.example/v1',
          apiTokenOverrideStatus: 'configured',
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'generic_openai_chat', version: 1 },
            reasoning: { mode: 'provider_default', effort: 'provider_default' }
          }
        },
        {
          ...storedSettings.models[0]!,
          id: 'deepseek-inherited',
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'deepseek_v4_chat', version: 1 },
            reasoning: { mode: 'enabled', effort: 'high' }
          }
        },
        {
          ...storedSettings.models[0]!,
          id: 'unknown-inherited',
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'future_vendor_chat', version: 7 },
            reasoning: { mode: 'provider_default', effort: 'provider_default' }
          }
        }
      ]
    }
    service.loadModelSettings.mockResolvedValue(settings)
    service.loadProviderProfileUiDescriptors.mockResolvedValue([
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
    ])

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(settings.apiUrl)
    await screen.getByRole('button', { name: 'change URL' }).click()

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    const savedModels = service.saveModelSettings.mock.calls[0]![0].models
    expect(
      savedModels.find((model) => model.id === 'generic-inherited')?.providerProfileUpdate
    ).toEqual({ kind: 'select_generic' })
    expect(
      savedModels.find((model) => model.id === 'generic-override')?.providerProfileUpdate
    ).toEqual({ kind: 'unchanged' })
    expect(
      savedModels.find((model) => model.id === 'deepseek-inherited')?.providerProfileUpdate
    ).toEqual({ kind: 'unchanged' })
    expect(
      savedModels.find((model) => model.id === 'unknown-inherited')?.providerProfileUpdate
    ).toEqual({ kind: 'unchanged' })
  })

  it('serializes queued saves against the latest Host revision', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings
      .mockImplementationOnce(async (draft) => ({
        ...draft,
        configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000002'
      }))
      .mockImplementationOnce(async (draft) => ({
        ...draft,
        configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000003'
      }))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)

    await screen.getByRole('button', { name: 'change URL' }).click()
    await screen.getByRole('button', { name: 'change URL' }).click()

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(2)
    expect(service.saveModelSettings.mock.calls[0]?.[1]).toBe(storedSettings.configurationRevision)
    expect(service.saveModelSettings.mock.calls[1]?.[1]).toBe(
      'model-settings-v1:00000000-0000-4000-8000-000000000002'
    )
  })

  it('exposes only the Host-projected Provider Profile descriptors', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.loadProviderProfileUiDescriptors.mockResolvedValue([
      {
        profileId: 'deepseek_v4_chat',
        profileVersion: 1,
        displayName: '深度求索 / DeepSeek（V4 Chat）',
        compatibleDialects: ['openai_chat_completions'],
        settingsKind: 'deepseek_v4_chat',
        selectable: true
      }
    ])

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect
      .element(screen.getByTestId('provider-profiles'))
      .toHaveTextContent('deepseek_v4_chat')
    expect(service.loadProviderProfileUiDescriptors).toHaveBeenCalledTimes(1)
  })

  it('exposes Host-projected vendor descriptors and delegates policy resolution to Host', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect
      .element(screen.getByTestId('provider-vendors'))
      .toHaveTextContent('generic,deepseek,moonshot')
    await screen.getByRole('button', { name: 'resolve vendor' }).click()
    await expect.poll(() => service.resolveProviderVendorModelPolicy.mock.calls.length).toBe(1)
    expect(service.resolveProviderVendorModelPolicy).toHaveBeenCalledWith({
      vendorId: 'moonshot',
      modelId: 'kimi-k3',
      dialect: 'openai_chat_completions'
    })
  })

  it('keeps model settings usable when the safe descriptor projection is unavailable', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.loadProviderProfileUiDescriptors.mockRejectedValue(
      new Error('descriptor projection unavailable')
    )

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)
    await expect.element(screen.getByTestId('provider-profiles')).toHaveTextContent('')
    expect(service.saveModelSettings).not.toHaveBeenCalled()
  })

  it('keeps the workspace usable when the optional vendor directory is unavailable', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.loadProviderVendorDescriptors.mockRejectedValue(new Error('mixed-version Host'))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)
    await expect.element(screen.getByTestId('provider-vendors')).toHaveTextContent('')
    expect(service.loadModelSettings).toHaveBeenCalledTimes(1)
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('replaces optimistic values with the Host-authoritative save result', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings.mockImplementation(async (settings: ModelSettingsSnapshot) => ({
      ...settings,
      apiUrl: 'https://host-normalized.example/v1'
    }))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)

    await screen.getByRole('button', { name: 'change URL' }).click()

    await expect
      .element(screen.getByTestId('api-url'))
      .toHaveTextContent('https://host-normalized.example/v1')
  })

  it('rolls back the latest optimistic update and shows a stable safe error', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings.mockRejectedValue(
      new Error('token=private-token; raw provider response')
    )

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)

    await screen.getByRole('button', { name: 'change URL' }).click()

    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)
    expect(service.showToast).toHaveBeenCalledWith('configuration.saveFailed', {
      durationMs: 5000
    })
    expect(JSON.stringify(service.showToast.mock.calls)).not.toContain('private-token')
    expect(JSON.stringify(service.showToast.mock.calls)).not.toContain('raw provider response')
  })

  it('reloads a newer Host snapshot instead of restoring stale settings after a save conflict', async () => {
    const concurrentlyUpdated = {
      ...storedSettings,
      configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000099',
      apiUrl: 'https://newer-window.example/v1/chat/completions'
    }
    service.loadModelSettings
      .mockResolvedValueOnce(storedSettings)
      .mockResolvedValue(concurrentlyUpdated)
    service.saveModelSettings.mockRejectedValue(new Error('model settings revision conflict'))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-url')).toHaveTextContent(storedSettings.apiUrl)

    await screen.getByRole('button', { name: 'change URL' }).click()

    await expect
      .element(screen.getByTestId('api-url'))
      .toHaveTextContent(concurrentlyUpdated.apiUrl)
    expect(service.loadModelSettings).toHaveBeenCalledTimes(2)
  })

  it('keeps a configured API token visible throughout a failed clear', async () => {
    let rejectSave!: (error: Error) => void
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings.mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          rejectSave = reject
        })
    )

    const screen = await render(
      <ModelSettingsProvider>
        <CredentialMutationProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('api-token-status')).toHaveTextContent('configured')
    await screen.getByRole('button', { name: 'clear API token' }).click()
    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings).toHaveBeenCalledWith(
      expect.objectContaining({ apiTokenMutation: { type: 'clear' } }),
      storedSettings.configurationRevision
    )
    await expect.element(screen.getByTestId('api-token-status')).toHaveTextContent('configured')

    rejectSave(new Error('save failed'))
    await expect.poll(() => service.showToast.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('api-token-status')).toHaveTextContent('configured')
  })

  it('keeps Tavily availability and search mode stable throughout a failed clear', async () => {
    let rejectSave!: (error: Error) => void
    service.loadModelSettings.mockResolvedValue({
      ...storedSettings,
      tavilyApiKeyStatus: 'configured'
    })
    service.saveModelSettings.mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          rejectSave = reject
        })
    )

    const screen = await render(
      <ModelSettingsProvider>
        <CredentialMutationProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('tavily-status')).toHaveTextContent('configured')
    await expect.element(screen.getByTestId('search-mode')).toHaveTextContent('auto')
    await screen.getByRole('button', { name: 'clear Tavily key' }).click()
    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        searchMode: 'disabled',
        tavilyApiKeyMutation: { type: 'clear' }
      }),
      storedSettings.configurationRevision
    )
    await expect.element(screen.getByTestId('tavily-status')).toHaveTextContent('configured')
    await expect.element(screen.getByTestId('search-mode')).toHaveTextContent('auto')

    rejectSave(new Error('save failed'))
    await expect.poll(() => service.showToast.mock.calls.length).toBe(1)
    await expect.element(screen.getByTestId('tavily-status')).toHaveTextContent('configured')
    await expect.element(screen.getByTestId('search-mode')).toHaveTextContent('auto')
  })

  it('keeps an override URL after clearing its credential and marks the model unavailable', async () => {
    const overrideSettings: ModelSettingsSnapshot = {
      ...storedSettings,
      models: [
        {
          ...storedSettings.models[0]!,
          id: 'override-model',
          apiUrlOverride: 'https://override.example/v1/chat/completions',
          apiTokenOverrideStatus: 'configured'
        }
      ]
    }
    service.loadModelSettings.mockResolvedValue(overrideSettings)
    service.saveModelSettings.mockImplementation(async (draft) => ({
      ...overrideSettings,
      searchMode: draft.searchMode,
      models: draft.models.map((savedModel) => ({
        ...savedModel,
        id: savedModel.id ?? 'unexpected-new-model',
        apiTokenOverrideStatus: 'missing' as const,
        apiTokenOverrideMutation: { type: 'keep' as const },
        providerProfileConfig: overrideSettings.models[0]!.providerProfileConfig
      }))
    }))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelOverrideCredentialProbe />
      </ModelSettingsProvider>
    )
    await expect
      .element(screen.getByTestId('override-enabled-models'))
      .toHaveTextContent('override-model')
    await screen.getByRole('button', { name: 'clear model override' }).click()
    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings.mock.calls[0]![0].models[0]).toMatchObject({
      apiUrlOverride: 'https://override.example/v1/chat/completions',
      apiTokenOverrideMutation: { type: 'clear' }
    })
    await expect.element(screen.getByTestId('override-status')).toHaveTextContent('missing')
    await expect
      .element(screen.getByTestId('override-url'))
      .toHaveTextContent('https://override.example/v1/chat/completions')
    await expect.element(screen.getByTestId('override-enabled-models')).toHaveTextContent('')
  })

  it('does not silently delete a model or toast when a display name edit collides', async () => {
    const settingsWithTwoModels: ModelSettingsSnapshot = {
      ...storedSettings,
      models: [
        {
          ...storedSettings.models[0]!,
          id: 'model-a',
          displayName: 'Model A'
        },
        {
          ...storedSettings.models[0]!,
          id: 'model-b',
          displayName: 'Model B'
        }
      ]
    }
    service.loadModelSettings.mockResolvedValue(settingsWithTwoModels)
    service.saveModelSettings.mockRejectedValue(
      new HostInvocationError({
        message: 'Model settings validation failed.',
        code: -32000,
        data: {
          kind: 'model_settings_validation',
          code: 'duplicate_display_name',
          displayName: 'Model B'
        }
      })
    )

    const screen = await render(
      <ModelSettingsProvider>
        <ModelRenameProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('model-ids')).toHaveTextContent('model-a,model-b')

    await screen.getByRole('button', { name: 'rename model' }).click()

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings.mock.calls[0]?.[0].models).toEqual([
      expect.objectContaining({ id: 'model-a', displayName: 'Model B' }),
      expect.objectContaining({ id: 'model-b', displayName: 'Model B' })
    ])
    await expect.element(screen.getByTestId('model-ids')).toHaveTextContent('model-a,model-b')
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('allows a repeated provider model ID and adopts the immutable ID assigned by Host', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings.mockImplementation(async (settings) => ({
      ...settings,
      models: settings.models.map((model) =>
        model.id === null
          ? {
              ...model,
              id: 'host-generated-id',
              displayName: 'New Model Normalized',
              providerProfileConfig: {
                schemaVersion: 1,
                profile: { id: 'generic_openai_chat', version: 1 },
                reasoning: { mode: 'provider_default', effort: 'provider_default' }
              }
            }
          : model
      )
    }))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelCreateProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('created-model-ids')).toHaveTextContent('stored-model')

    await screen.getByRole('button', { name: 'create model' }).click()

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings.mock.calls[0]![0].models.at(-1)).toMatchObject({
      id: null,
      displayName: 'New Model',
      providerModelId: 'provider-stored-model'
    })
    await expect
      .element(screen.getByTestId('created-model-ids'))
      .toHaveTextContent('stored-model,host-generated-id')
    await expect
      .element(screen.getByTestId('created-model-result'))
      .toHaveTextContent('host-generated-id')
    await expect
      .element(screen.getByTestId('created-model-display-names'))
      .toHaveTextContent('Stored Model,New Model Normalized')
  })

  it('rejects an ambiguous Host response containing more than one new immutable ID', async () => {
    service.loadModelSettings.mockResolvedValue(storedSettings)
    service.saveModelSettings.mockImplementation(async (settings) => {
      const draft = settings.models.at(-1)!
      const providerProfileConfig = {
        schemaVersion: 1 as const,
        profile: { id: 'generic_openai_chat', version: 1 },
        reasoning: { mode: 'provider_default' as const, effort: 'provider_default' as const }
      }
      return {
        ...settings,
        models: [
          ...settings.models.slice(0, -1),
          { ...draft, id: 'host-generated-a', providerProfileConfig },
          { ...draft, id: 'host-generated-b', displayName: 'Another Model', providerProfileConfig }
        ]
      }
    })

    const screen = await render(
      <ModelSettingsProvider>
        <ModelCreateProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('created-model-ids')).toHaveTextContent('stored-model')

    await screen.getByRole('button', { name: 'create model' }).click()

    await expect
      .element(screen.getByTestId('created-model-result'))
      .toHaveTextContent('error:Host did not return the saved model')
  })

  it('recovers from a transient core-server read failure', async () => {
    service.loadModelSettings
      .mockRejectedValueOnce(new Error('core-server restarted'))
      .mockResolvedValueOnce(storedSettings)

    const screen = await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.element(screen.getByTestId('enabled-models')).toHaveTextContent('stored-model')
    expect(service.loadModelSettings).toHaveBeenCalledTimes(2)
    expect(service.saveModelSettings).not.toHaveBeenCalled()
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('initializes an empty model catalog only after a successful first-run read', async () => {
    service.loadModelSettings.mockResolvedValue(null)

    await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        apiTokenMutation: { type: 'keep' },
        apiUrl: '',
        tavilyApiKeyMutation: { type: 'keep' },
        models: []
      }),
      null
    )
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('never overwrites SQLite defaults when every read attempt fails', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    service.loadModelSettings.mockRejectedValue(new Error('storage unavailable'))

    await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.poll(() => service.showToast.mock.calls.length).toBe(1)
    expect(service.loadModelSettings).toHaveBeenCalledTimes(3)
    expect(service.saveModelSettings).not.toHaveBeenCalled()
    expect(service.showToast).toHaveBeenCalledWith('configuration.loadFailed', { durationMs: 5000 })
    consoleError.mockRestore()
  })
})
