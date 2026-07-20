import { StrictMode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelSettingsSnapshot } from '../../features/storage/storageClient'

const service = vi.hoisted(() => ({
  loadModelSettings: vi.fn(),
  saveModelSettings: vi.fn(),
  showToast: vi.fn()
}))

vi.mock('../../features/storage/storageClient', () => ({
  loadModelSettings: service.loadModelSettings,
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
  const { apiUrl, enabledModels, setApiUrl } = useModelSettings()

  return (
    <div>
      <span data-testid="api-url">{apiUrl}</span>
      <span data-testid="enabled-models">{enabledModels.map((model) => model.id).join(',')}</span>
      <button type="button" onClick={() => setApiUrl('https://changed.example/v1')}>
        change URL
      </button>
    </div>
  )
}

beforeEach(() => {
  service.loadModelSettings.mockReset()
  service.saveModelSettings.mockReset().mockResolvedValue(undefined)
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
      apiUrl: 'https://changed.example/v1'
    })
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
