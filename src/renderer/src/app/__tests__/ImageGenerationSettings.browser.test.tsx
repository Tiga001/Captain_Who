// Browser coverage for image-generation configuration CAS, secrets, and recovery behavior.
import { HostInvocationError } from '@mycopilot/host-api'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'
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
      <span data-testid="reload-credential-status">
        {workflow.state.status === 'ready'
          ? workflow.state.configuration.credentialStatus
          : 'loading'}
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
  return {
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
    configuration: value
  }
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
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
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
    schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
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

  it('loads only a configured status and never exposes the existing API key', async () => {
    const pending = deferred<ImageGenerationGetConfigurationOutput>()
    service.getConfiguration.mockReturnValueOnce(pending.promise)
    const screen = await render(<ImageGenerationSettings />)

    await expect.element(screen.getByText('configuration.imageGeneration.loading')).toBeVisible()
    pending.resolve(configurationOutput(configuration()))
    await expect
      .element(screen.getByRole('textbox', { name: 'configuration.imageGeneration.endpointUrl' }))
      .toBeVisible()

    expect(
      screen.container.querySelector('input[aria-label="configuration.imageGeneration.apiKey"]')
    ).toBeNull()
    expect(screen.container.textContent).toContain('configuration.credential.configured')
    expect(screen.container.textContent).not.toContain('existing-api-key')
    await screen.getByRole('button', { name: 'configuration.credential.replace' }).click()
    const secretInput = screen.getByLabelText('configuration.imageGeneration.apiKey')
    await expect.element(secretInput).toHaveValue('')
    await screen.getByRole('button', { name: 'configuration.showSecretValue' }).click()
    await expect.element(secretInput).toHaveAttribute('type', 'text')
    expect(screen.container.textContent).not.toContain(
      'configuration.imageGeneration.enabledDescription'
    )
    expect(screen.container.textContent).not.toContain('configuration.imageGeneration.outputSize')
    expect(screen.container.textContent).not.toContain('configuration.imageGeneration.status')
    expect(
      screen.container
        .querySelector('input[aria-label="configuration.imageGeneration.apiKey"]')
        ?.closest('.settings-list-row__control')?.children
    ).toHaveLength(1)

    const textToImage = screen.getByRole('switch', {
      name: 'configuration.imageGeneration.textToImage'
    })
    await expect.element(textToImage).toBeDisabled()
    await expect.element(textToImage).toHaveAttribute('aria-checked', 'true')
  })

  it('replaces an edited credential, retains it after saving, then keeps the new baseline', async () => {
    const screen = await render(<ImageGenerationSettings />)
    await expect
      .element(screen.getByRole('heading', { name: 'configuration.imageGeneration.title' }))
      .toBeVisible()

    await screen.getByRole('button', { name: 'configuration.credential.replace' }).click()
    const secretInput = screen.getByLabelText('configuration.imageGeneration.apiKey')
    await secretInput.fill('top-secret')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()

    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenCalledWith(
        expect.objectContaining({ credentialMutation: { type: 'replace', value: 'top-secret' } })
      )
    })
    expect(screen.container.textContent).not.toContain('top-secret')
    expect(screen.container.textContent).toContain('configuration.credential.configured')
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()
    await vi.waitFor(() => expect(service.updateConfiguration).toHaveBeenCalledTimes(2))
    expect(service.updateConfiguration).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ credentialMutation: { type: 'keep' } })
    )
    expect(
      screen.container.querySelector('.image-generation-settings__feedback')?.textContent
    ).toBe('')
  })

  it('keeps the stored credential when the loaded secret stays unchanged', async () => {
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

  it('clears an existing credential explicitly and treats the empty value as the new baseline', async () => {
    service.updateConfiguration
      .mockResolvedValueOnce({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        outcome: 'updated',
        configuration: configuration({ credentialStatus: 'missing', revision: 'revision-2' })
      })
      .mockResolvedValueOnce({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        outcome: 'alreadyCurrent',
        configuration: configuration({ credentialStatus: 'missing', revision: 'revision-2' })
      })
    const screen = await render(<ImageGenerationSettings />)
    await screen.getByRole('button', { name: 'configuration.credential.clear' }).click()
    await screen.getByRole('button', { name: 'configuration.credential.clearConfirm' }).click()
    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()
    await vi.waitFor(() => {
      expect(service.updateConfiguration).toHaveBeenNthCalledWith(
        1,
        expect.objectContaining({ credentialMutation: { type: 'clear' } })
      )
    })
    await expect
      .element(screen.getByLabelText('configuration.imageGeneration.apiKey'))
      .toHaveValue('')

    await screen.getByRole('button', { name: 'configuration.imageGeneration.save' }).click()
    await vi.waitFor(() => expect(service.updateConfiguration).toHaveBeenCalledTimes(2))
    expect(service.updateConfiguration).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ credentialMutation: { type: 'keep' } })
    )
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
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      outcome: 'updated',
      configuration: updated
    })
    service.setEnabled.mockResolvedValueOnce({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
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
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      expectedRevision: 'revision-2',
      enabled: true
    })
    expect(service.updateConfiguration.mock.invocationCallOrder[0]).toBeLessThan(
      service.setEnabled.mock.invocationCallOrder[0]
    )
    expect(screen.container.textContent).toContain('configuration.credential.configured')
    expect(screen.container.textContent).not.toContain('existing-api-key')
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
    await expect
      .element(screen.getByTestId('reload-credential-status'))
      .toHaveTextContent('configured')
    older.resolve(
      configurationOutput(configuration({ modelId: 'older-model', revision: 'revision-older' }))
    )

    await expect.element(screen.getByText('newer-model')).toBeVisible()
    await expect
      .element(screen.getByTestId('reload-credential-status'))
      .toHaveTextContent('configured')
    expect(screen.container.textContent).not.toContain('older-model')
  })
})
