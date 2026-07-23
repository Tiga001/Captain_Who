// Browser coverage for image-generation configuration CAS, secrets, and recovery behavior.
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  ImageGenerationConfiguration,
  ImageGenerationGetConfigurationOutput
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  getConfiguration: vi.fn(),
  setEnabled: vi.fn(),
  updateConfiguration: vi.fn()
}))

vi.mock('../../features/imageGeneration/configuration/imageGenerationClient', () => ({
  getImageGenerationConfiguration: service.getConfiguration,
  setImageGenerationEnabled: service.setEnabled,
  updateImageGenerationConfiguration: service.updateConfiguration
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const [{ ImageGenerationSettings }, { useImageGenerationConfiguration }] = await Promise.all([
  import('../../features/settings/pages/configuration/ImageGenerationSettings'),
  import('../../features/imageGeneration/configuration/useImageGenerationConfiguration')
])

function ReloadHarness() {
  const workflow = useImageGenerationConfiguration()
  return (
    <div>
      <button onClick={() => void workflow.load()} type="button">
        reload
      </button>
      <span>
        {workflow.state.status === 'ready' ? workflow.state.configuration.modelId : 'loading'}
      </span>
    </div>
  )
}

function configuration(
  overrides: Partial<ImageGenerationConfiguration> = {}
): ImageGenerationConfiguration {
  return {
    adapterId: 'smartmlSeedream',
    endpointUrl: 'https://images.example/v1',
    modelId: 'seedream-model',
    capabilities: { textToImage: true, imageToImage: false },
    defaults: { sizePreset: '2K', watermark: false },
    credentialStatus: 'configured',
    enabled: false,
    readiness: 'disabled',
    revision: 'revision-1',
    ...overrides
  }
}

function configurationOutput(
  value: ImageGenerationConfiguration
): ImageGenerationGetConfigurationOutput {
  return { schemaVersion: 1, configuration: value }
}

function deferred<Value>() {
  let resolve!: (value: Value) => void
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve }
}

beforeEach(() => {
  vi.clearAllMocks()
  service.getConfiguration.mockResolvedValue(configurationOutput(configuration()))
  service.updateConfiguration.mockImplementation(async (input) => ({
    schemaVersion: 1,
    outcome: 'updated',
    configuration: configuration({
      endpointUrl: input.endpointUrl,
      modelId: input.modelId,
      capabilities: input.capabilities,
      defaults: input.defaults,
      credentialStatus: input.credentialMutation.type === 'clear' ? 'missing' : 'configured',
      revision: 'revision-2'
    })
  }))
  service.setEnabled.mockImplementation(async (input) => ({
    schemaVersion: 1,
    outcome: 'updated',
    configuration: configuration({
      enabled: input.enabled,
      readiness: input.enabled ? 'readyUnverified' : 'disabled',
      revision: 'revision-3'
    })
  }))
})

describe('ImageGenerationSettings', () => {
  it('shows a load error and retries the authoritative request', async () => {
    service.getConfiguration
      .mockRejectedValueOnce(new Error('unavailable'))
      .mockResolvedValueOnce(configurationOutput(configuration()))
    const screen = await render(<ImageGenerationSettings />)

    await expect
      .element(screen.getByText('configuration.imageGeneration.error.loadFailed'))
      .toBeVisible()
    await screen.getByRole('button', { name: 'configuration.imageGeneration.retry' }).click()
    await expect
      .element(screen.getByRole('textbox', { name: 'configuration.imageGeneration.endpointUrl' }))
      .toBeVisible()
    expect(service.getConfiguration).toHaveBeenCalledTimes(2)
  })

  it('shows loading state and never echoes an existing API Key', async () => {
    const pending = deferred<ImageGenerationGetConfigurationOutput>()
    service.getConfiguration.mockReturnValueOnce(pending.promise)
    const screen = await render(<ImageGenerationSettings />)

    await expect.element(screen.getByText('configuration.imageGeneration.loading')).toBeVisible()
    pending.resolve(configurationOutput(configuration()))
    await expect
      .element(screen.getByRole('textbox', { name: 'configuration.imageGeneration.endpointUrl' }))
      .toBeVisible()

    const secretInput = screen.container.querySelector<HTMLInputElement>(
      'input[aria-label="configuration.imageGeneration.apiKey"]'
    )
    expect(secretInput?.type).toBe('password')
    expect(secretInput?.value).toBe('')
    expect(secretInput?.placeholder).toBe('\u2022'.repeat(18))
    expect(screen.container.textContent).not.toContain('top-secret')
    expect(screen.container.textContent).not.toContain(
      'configuration.imageGeneration.credential.configured'
    )
    expect(screen.container.textContent).not.toContain(
      'configuration.imageGeneration.enabledDescription'
    )
    expect(screen.container.textContent).not.toContain('configuration.imageGeneration.outputSize')
    expect(screen.container.textContent).not.toContain('configuration.imageGeneration.status')
    expect(secretInput?.closest('.settings-list-row__control')?.children).toHaveLength(1)

    const textToImage = screen.getByRole('switch', {
      name: 'configuration.imageGeneration.textToImage'
    })
    await expect.element(textToImage).toBeDisabled()
    await expect.element(textToImage).toHaveAttribute('aria-checked', 'true')
  })

  it('replaces a credential only after explicit input and clears plaintext after saving', async () => {
    const screen = await render(<ImageGenerationSettings />)
    await expect
      .element(screen.getByRole('heading', { name: 'configuration.imageGeneration.title' }))
      .toBeVisible()

    const secretInput = screen.getByLabelText('configuration.imageGeneration.apiKey')
    await secretInput.fill('top-secret')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenCalledWith(
        expect.objectContaining({ credentialMutation: { type: 'replace', value: 'top-secret' } })
      )
    })
    await expect.element(secretInput).toHaveValue('')
    await expect.element(secretInput).toHaveAttribute('placeholder', '\u2022'.repeat(18))
    expect(screen.container.textContent).not.toContain('top-secret')
  })

  it('keeps the stored credential when the secret field stays empty', async () => {
    const screen = await render(<ImageGenerationSettings />)
    await screen
      .getByRole('textbox', { name: 'configuration.imageGeneration.modelId' })
      .fill('new-model')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenCalledWith(
        expect.objectContaining({ credentialMutation: { type: 'keep' } })
      )
    })
  })

  it('allows image-to-image configuration while keeping text-to-image mandatory', async () => {
    const screen = await render(<ImageGenerationSettings />)
    await screen.getByRole('switch', { name: 'configuration.imageGeneration.imageToImage' }).click()
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenCalledWith(
        expect.objectContaining({
          capabilities: { textToImage: true, imageToImage: true },
          adapterId: 'smartmlSeedream',
          defaults: expect.objectContaining({ sizePreset: '2K' })
        })
      )
    })
  })

  it('saves dirty fields before enabling and uses the returned revision serially', async () => {
    const updated = configuration({ endpointUrl: 'https://new.example/v1', revision: 'revision-2' })
    service.updateConfiguration.mockResolvedValueOnce({
      schemaVersion: 1,
      outcome: 'updated',
      configuration: updated
    })
    service.setEnabled.mockResolvedValueOnce({
      schemaVersion: 1,
      outcome: 'updated',
      configuration: configuration({
        ...updated,
        enabled: true,
        readiness: 'readyUnverified',
        revision: 'revision-3'
      })
    })
    const screen = await render(<ImageGenerationSettings />)
    await screen
      .getByRole('textbox', { name: 'configuration.imageGeneration.endpointUrl' })
      .fill('https://new.example/v1')
    await screen.getByRole('switch', { name: 'configuration.imageGeneration.enabled' }).click()

    await vi.waitFor(() => expect(service.setEnabled).toHaveBeenCalledTimes(1))
    expect(service.updateConfiguration).toHaveBeenCalledTimes(1)
    expect(service.setEnabled).toHaveBeenCalledWith({
      schemaVersion: 1,
      expectedRevision: 'revision-2',
      enabled: true
    })
    expect(service.updateConfiguration.mock.invocationCallOrder[0]).toBeLessThan(
      service.setEnabled.mock.invocationCallOrder[0]
    )
  })

  it.each([
    ['revisionConflict', false],
    ['commitIndeterminate', true]
  ] as const)('refreshes authoritative state for %s without blind retry', async (code, changed) => {
    const latest = configuration({ modelId: 'authoritative-model', revision: 'revision-latest' })
    service.getConfiguration
      .mockResolvedValueOnce(configurationOutput(configuration()))
      .mockResolvedValueOnce(configurationOutput(latest))
    service.updateConfiguration.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'structured failure',
        data: {
          type: 'imageGenerationConfiguration',
          operation: 'updateConfiguration',
          code,
          recovery: 'refreshConfiguration',
          ...(changed ? { configurationMayHaveChanged: true } : {})
        }
      })
    )

    const screen = await render(<ImageGenerationSettings />)
    await screen
      .getByRole('textbox', { name: 'configuration.imageGeneration.modelId' })
      .fill('draft-model')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await expect
      .element(screen.getByRole('textbox', { name: 'configuration.imageGeneration.modelId' }))
      .toHaveValue('authoritative-model')
    expect(service.updateConfiguration).toHaveBeenCalledTimes(1)
    expect(service.getConfiguration).toHaveBeenCalledTimes(2)
  })

  it('does not claim an indeterminate mutation was confirmed when the refresh also fails', async () => {
    service.getConfiguration
      .mockResolvedValueOnce(configurationOutput(configuration()))
      .mockRejectedValueOnce(new Error('refresh unavailable'))
    service.updateConfiguration.mockRejectedValueOnce(
      new HostInvocationError({
        message: 'structured failure',
        data: {
          type: 'imageGenerationConfiguration',
          operation: 'updateConfiguration',
          code: 'commitIndeterminate',
          recovery: 'refreshConfiguration',
          configurationMayHaveChanged: true
        }
      })
    )

    const screen = await render(<ImageGenerationSettings />)
    await screen
      .getByRole('textbox', { name: 'configuration.imageGeneration.modelId' })
      .fill('draft-model')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await expect
      .element(screen.getByText('configuration.imageGeneration.error.unavailable'))
      .toBeVisible()
    expect(screen.container.textContent).not.toContain(
      'configuration.imageGeneration.error.commitIndeterminateRefreshed'
    )
    expect(service.updateConfiguration).toHaveBeenCalledTimes(1)
    expect(service.getConfiguration).toHaveBeenCalledTimes(2)
  })

  it('keeps Save available when values are unchanged and relies on backend idempotency', async () => {
    const screen = await render(<ImageGenerationSettings />)
    const saveButton = screen.getByRole('button', { name: 'configuration.imageGeneration.save' })
    await expect.element(saveButton).toBeEnabled()
    await saveButton.click()

    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenCalledWith(
        expect.objectContaining({
          expectedRevision: 'revision-1',
          credentialMutation: { type: 'keep' }
        })
      )
    })
  })

  it('prevents an older configuration response from overwriting a newer reload', async () => {
    const older = deferred<ImageGenerationGetConfigurationOutput>()
    const newer = deferred<ImageGenerationGetConfigurationOutput>()
    service.getConfiguration.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise)
    const screen = await render(<ReloadHarness />)
    await expect.poll(() => service.getConfiguration.mock.calls.length).toBe(1)
    await screen.getByRole('button', { name: 'reload' }).click()

    newer.resolve(
      configurationOutput(configuration({ modelId: 'newer-model', revision: 'revision-newer' }))
    )
    await expect.element(screen.getByText('newer-model')).toBeVisible()
    older.resolve(
      configurationOutput(configuration({ modelId: 'older-model', revision: 'revision-older' }))
    )

    await expect.element(screen.getByText('newer-model')).toBeVisible()
    expect(screen.container.textContent).not.toContain('older-model')
  })
})
