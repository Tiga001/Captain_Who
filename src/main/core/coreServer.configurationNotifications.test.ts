import { beforeEach, describe, expect, it, vi } from 'vitest'
import { IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION } from '@mycopilot/protocol'

const rpc = vi.hoisted(() => ({ request: vi.fn(), onNotification: vi.fn(), start: vi.fn() }))
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpc.request
    readonly onNotification = rpc.onNotification
    readonly start = rpc.start
  }
}))

import { CoreServer } from './coreServer'

const imageInput = {
  schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
  expectedRevision: 'image-generation:v1:1',
  enabled: false
}
const modelInput = {
  expectedRevision: null,
  apiUrl: '',
  apiTokenMutation: { type: 'keep' as const },
  searchMode: 'auto',
  tavilyApiKeyMutation: { type: 'keep' as const },
  models: []
}

describe('Host configuration invalidations', () => {
  beforeEach(() => {
    rpc.request.mockReset()
    rpc.onNotification.mockReset().mockReturnValue(() => undefined)
    rpc.start.mockReset()
  })

  it('invalidates only the changed domain after authoritative model and image writes', async () => {
    const server = new CoreServer()
    const models = vi.fn()
    const images = vi.fn()
    const builtins = vi.fn()
    server.onConfigurationInvalidated('modelSettings', models)
    server.onConfigurationInvalidated('imageGeneration', images)
    server.onConfigurationInvalidated('builtinCapabilities', builtins)
    rpc.request.mockResolvedValueOnce({
      configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
      apiUrl: '',
      apiTokenStatus: 'missing',
      searchMode: 'auto',
      tavilyApiKeyStatus: 'missing',
      models: []
    })
    await server.saveModelSettings(modelInput)
    expect(models.mock.calls).toEqual([[]])
    expect(images).not.toHaveBeenCalled()
    expect(builtins).not.toHaveBeenCalled()

    rpc.request.mockResolvedValueOnce({
      schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
      outcome: 'updated',
      configuration: {
        adapterId: 'smartmlSeedream',
        endpointUrl: 'https://example.test/v1/images/generations',
        modelId: 'model',
        capabilities: { textToImage: true, imageToImage: true },
        defaults: { sizePreset: '2K', watermark: true },
        credentialStatus: 'configured',
        enabled: false,
        readiness: 'disabled',
        revision: 'image-generation:v1:2'
      }
    })
    await server.setImageGenerationEnabled(imageInput)
    expect(images.mock.calls).toEqual([[]])
    expect(models).toHaveBeenCalledTimes(1)
    expect(builtins).not.toHaveBeenCalled()
  })

  it('invalidates after uncertain mutation outcomes, including the image Skill alias', async () => {
    const server = new CoreServer()
    const models = vi.fn()
    const images = vi.fn()
    const builtins = vi.fn()
    server.onConfigurationInvalidated('modelSettings', models)
    server.onConfigurationInvalidated('imageGeneration', images)
    server.onConfigurationInvalidated('builtinCapabilities', builtins)
    const uncertain = new Error('transport disconnected after possible commit')
    rpc.request.mockRejectedValue(uncertain)

    await expect(server.saveModelSettings(modelInput)).rejects.toBe(uncertain)
    await expect(server.setImageGenerationEnabled(imageInput)).rejects.toBe(uncertain)
    await expect(
      server.updateImageGenerationConfiguration({
        schemaVersion: IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION,
        expectedRevision: imageInput.expectedRevision,
        adapterId: 'smartmlSeedream',
        endpointUrl: 'https://example.test/v1/images/generations',
        modelId: 'model',
        capabilities: { textToImage: true, imageToImage: true },
        defaults: { sizePreset: '2K', watermark: true },
        credentialMutation: { type: 'keep' }
      })
    ).rejects.toBe(uncertain)
    await expect(
      server.setSkillEnabled({
        skillId: 'bundled:application:image-generation',
        expectedStateRevision: 'revision',
        enabled: false
      })
    ).rejects.toBe(uncertain)
    await expect(
      server.setSkillEnabled({
        skillId: 'another-skill',
        expectedStateRevision: 'r',
        enabled: false
      })
    ).rejects.toBe(uncertain)
    await expect(
      server.setMcpBuiltinCapabilityAllowed({
        schemaVersion: 1,
        capabilityId: 'browser_automation',
        expectedPolicyRevision: 0,
        allowed: false
      })
    ).rejects.toBe(uncertain)
    expect(models.mock.calls).toEqual([[]])
    expect(images.mock.calls).toEqual([[], [], []])
    expect(builtins.mock.calls).toEqual([[]])
  })

  it('requeries all domains on Core startup/reconnect and removes listeners exactly', () => {
    const notifications = new Map<string, (value: unknown) => void>()
    rpc.onNotification.mockImplementation((method, handler) => {
      notifications.set(method, handler)
      return () => undefined
    })
    const server = new CoreServer()
    const handlers = [vi.fn(), vi.fn(), vi.fn()]
    const unsubscribers = (
      ['modelSettings', 'imageGeneration', 'builtinCapabilities'] as const
    ).map((domain, index) => server.onConfigurationInvalidated(domain, handlers[index]))
    server.start()
    const event = { schemaVersion: 1, reason: 'core_started', lastSequence: 1, occurredAt: 1 }
    notifications.get('notification.resync')?.(event)
    notifications.get('notification.resync')?.({ ...event, occurredAt: 2 })
    for (const handler of handlers) expect(handler.mock.calls).toEqual([[], []])
    unsubscribers.forEach((unsubscribe) => unsubscribe())
    notifications.get('notification.resync')?.(event)
    for (const handler of handlers) expect(handler).toHaveBeenCalledTimes(2)
    expect(rpc.request).not.toHaveBeenCalled()
  })
})
