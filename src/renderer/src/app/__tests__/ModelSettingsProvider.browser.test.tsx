import { StrictMode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelSettingsSnapshot } from '../../features/storage/storageClient'

const service = vi.hoisted(() => ({
  loadModelSettings: vi.fn(),
  loadProviderProfileUiDescriptors: vi.fn(),
  saveModelSettings: vi.fn(),
  showToast: vi.fn()
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadModelSettings: service.loadModelSettings,
  loadProviderProfileUiDescriptors: service.loadProviderProfileUiDescriptors,
  saveModelSettings: service.saveModelSettings
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { ModelSettingsProvider, useModelSettings } =
  await import('../../config/ModelSettingsProvider')

const storedSettings: ModelSettingsSnapshot = {
  apiUrl: 'https://provider.example/v1/chat/completions',
  apiToken: 'stored-token',
  searchMode: 'auto',
  tavilyApiKey: '',
  models: [
    {
      id: 'stored-model',
      displayName: 'Stored Model',
      supportsImage: false,
      inputPrice: '0',
      outputPrice: '0',
      enabled: true
    }
  ]
}

function ModelSettingsProbe() {
  const { apiUrl, enabledModels, providerProfileDescriptors, setApiUrl } = useModelSettings()

  return (
    <div>
      <span data-testid="api-url">{apiUrl}</span>
      <span data-testid="enabled-models">{enabledModels.map((model) => model.id).join(',')}</span>
      <span data-testid="provider-profiles">
        {providerProfileDescriptors.map((profile) => profile.profileId).join(',')}
      </span>
      <button type="button" onClick={() => setApiUrl('https://api.anthropic.com/v1/messages')}>
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
            void upsertModel({ ...source, id: 'model-b' }, 'model-a').catch(() => undefined)
          }
        }}
      >
        rename model
      </button>
    </div>
  )
}

beforeEach(() => {
  service.loadModelSettings.mockReset()
  service.loadProviderProfileUiDescriptors.mockReset().mockResolvedValue([])
  service.saveModelSettings.mockReset().mockImplementation(async (settings) => settings)
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
    expect(service.saveModelSettings).toHaveBeenLastCalledWith({
      ...storedSettings,
      apiUrl: 'https://api.anthropic.com/v1/messages',
      models: [
        {
          ...storedSettings.models[0],
          providerProfileUpdate: { kind: 'select_generic' }
        }
      ]
    })
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
          apiTokenOverride: 'override-token',
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
    ).toBeUndefined()
    expect(
      savedModels.find((model) => model.id === 'deepseek-inherited')?.providerProfileUpdate
    ).toBeUndefined()
    expect(
      savedModels.find((model) => model.id === 'unknown-inherited')?.providerProfileUpdate
    ).toBeUndefined()
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

  it('does not silently delete a colliding model during rename', async () => {
    const settingsWithTwoModels: ModelSettingsSnapshot = {
      ...storedSettings,
      models: [
        { ...storedSettings.models[0]!, id: 'model-a', displayName: 'Model A' },
        { ...storedSettings.models[0]!, id: 'model-b', displayName: 'Model B' }
      ]
    }
    service.loadModelSettings.mockResolvedValue(settingsWithTwoModels)
    service.saveModelSettings.mockRejectedValue(new Error('duplicate model id'))

    const screen = await render(
      <ModelSettingsProvider>
        <ModelRenameProbe />
      </ModelSettingsProvider>
    )
    await expect.element(screen.getByTestId('model-ids')).toHaveTextContent('model-a,model-b')

    await screen.getByRole('button', { name: 'rename model' }).click()

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings.mock.calls[0]?.[0].models).toEqual([
      expect.objectContaining({ id: 'model-b', previousModelId: 'model-a' }),
      expect.objectContaining({ id: 'model-b', displayName: 'Model B' })
    ])
    await expect.element(screen.getByTestId('model-ids')).toHaveTextContent('model-a,model-b')
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

  it('initializes defaults only after a successful first-run read', async () => {
    service.loadModelSettings.mockResolvedValue(null)

    await render(
      <ModelSettingsProvider>
        <ModelSettingsProbe />
      </ModelSettingsProvider>
    )

    await expect.poll(() => service.saveModelSettings.mock.calls.length).toBe(1)
    expect(service.saveModelSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        apiToken: '',
        apiUrl: '',
        models: expect.arrayContaining([expect.objectContaining({ id: 'gpt-5.5' })])
      })
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
    expect(service.showToast).toHaveBeenCalledWith(
      'configuration.loadFailed: storage unavailable',
      { durationMs: 5000 }
    )
    consoleError.mockRestore()
  })
})
