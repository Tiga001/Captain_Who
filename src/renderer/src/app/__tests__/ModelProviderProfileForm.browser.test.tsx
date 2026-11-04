import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  ProviderProfileUiDescriptor,
  ProviderVendorDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderVendorModelPolicyInput
} from '@mycopilot/protocol'
import type { ModelConfig } from '../../config/modelConfig'
import {
  SettingsSearchNavigationProvider,
  type SettingsNavigationTarget
} from '../../features/settings/settingsSearchNavigation'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => {
      if (key === 'configuration.duplicateDisplayName') return `${key}:{displayName}`
      if (key === 'configuration.invalidContextCapacity') {
        return `${key}:{displayName}:{contextWindowTokens}:{reservedOutputTokens}:{safetyMarginTokens}`
      }
      if (key === 'configuration.contextCapacityAcknowledge') return '知道了'
      return key
    }
  })
}))

const { ModelForm } = await import('../../features/settings/pages/configuration/ModelForm')

const profileDescriptors: ProviderProfileUiDescriptor[] = [
  {
    profileId: 'generic_openai_chat',
    profileVersion: 1,
    displayName: 'Generic OpenAI Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: true
  },
  {
    profileId: 'deepseek_v4_1_flash_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4.1 Flash Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'deepseek_v4_pro_0813_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4 Pro 0813 Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k3_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K3',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k2_7_code_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K2.7 Code',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  },
  {
    profileId: 'moonshot_k2_6_chat',
    profileVersion: 1,
    displayName: 'Moonshot Kimi K2.6',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'none',
    selectable: false
  }
]

const vendorDescriptors: ProviderVendorDescriptor[] = [
  {
    vendorId: 'generic',
    displayName: 'Generic internal label',
    selectable: true,
    fixedApiUrl: null,
    modelIdPolicy: { kind: 'free_text' }
  },
  {
    vendorId: 'deepseek',
    displayName: 'DeepSeek internal label',
    selectable: true,
    fixedApiUrl: 'https://api.deepseek.com/chat/completions',
    modelIdPolicy: { kind: 'fixed', modelIds: ['deepseek-flash', 'deepseek-v4-pro'] }
  },
  {
    vendorId: 'moonshot',
    displayName: 'Moonshot internal label',
    selectable: true,
    fixedApiUrl: 'https://api.moonshot.cn/v1/chat/completions',
    modelIdPolicy: {
      kind: 'fixed',
      modelIds: [
        'kimi-k3',
        'kimi-k3.1-pro',
        'kimi-k3.1-pro-highspeed',
        'kimi-k2.7-code',
        'kimi-k2.7-code-highspeed',
        'kimi-k2.6'
      ]
    }
  },
  {
    vendorId: 'future_vendor.v1',
    displayName: 'Future internal label',
    selectable: true,
    fixedApiUrl: null,
    modelIdPolicy: { kind: 'free_text' }
  }
]

function genericPolicy(input: ProviderVendorModelPolicyInput): ProviderVendorModelPolicyDescriptor {
  return {
    status: 'supported',
    vendorId: 'generic',
    modelFamily:
      input.dialect === 'anthropic_messages' ? 'generic_anthropic_messages' : 'generic_openai_chat',
    settingsKind: 'none',
    imageInput: 'user_configurable',
    settings: { kind: 'generic', defaultSettings: { kind: 'generic' } }
  }
}

function resolvePolicy(
  input: ProviderVendorModelPolicyInput
): Promise<ProviderVendorModelPolicyDescriptor> {
  if (input.vendorId === 'generic') return Promise.resolve(genericPolicy(input))
  if (input.dialect !== 'openai_chat_completions') {
    return Promise.resolve({
      status: 'unsupported',
      vendorId: input.vendorId,
      reason: 'unsupported_dialect'
    })
  }
  if (input.vendorId === 'deepseek' && input.modelId === 'deepseek-flash') {
    return Promise.resolve({
      status: 'supported',
      vendorId: 'deepseek',
      modelFamily: 'deepseek_flash_chat',
      settingsKind: 'deepseek',
      imageInput: 'supported',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoningModes: ['provider_default', 'enabled', 'disabled'],
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: {
          kind: 'deepseek_flash_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }
      }
    })
  }
  if (input.vendorId === 'deepseek' && input.modelId === 'deepseek-v4-pro') {
    return Promise.resolve({
      status: 'supported',
      vendorId: 'deepseek',
      modelFamily: 'deepseek_pro_chat',
      settingsKind: 'deepseek',
      imageInput: 'unsupported',
      settings: {
        kind: 'deepseek_pro_chat',
        reasoningModes: ['provider_default', 'enabled', 'disabled'],
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: {
          kind: 'deepseek_pro_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }
      }
    })
  }
  if (
    input.vendorId === 'moonshot' &&
    ['kimi-k3', 'kimi-k3.1-pro', 'kimi-k3.1-pro-highspeed'].includes(input.modelId)
  ) {
    return Promise.resolve({
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
  }
  if (
    input.vendorId === 'moonshot' &&
    (input.modelId === 'kimi-k2.7-code' || input.modelId === 'kimi-k2.7-code-highspeed')
  ) {
    return Promise.resolve({
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k2_7_code_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k2_7_code_chat',
        defaultSettings: { kind: 'moonshot_k2_7_code_chat' }
      }
    })
  }
  if (input.vendorId === 'moonshot' && input.modelId === 'kimi-k2.6') {
    return Promise.resolve({
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k2_6_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k2_6_chat',
        thinkingModes: ['provider_default', 'enabled', 'disabled', 'enabled_keep_all'],
        defaultSettings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }
      }
    })
  }
  return Promise.resolve({
    status: 'unsupported',
    vendorId: input.vendorId,
    reason: 'unsupported_model'
  })
}

function deferred<Value>() {
  let resolve!: (value: Value) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

const model: ModelConfig = {
  id: 'model-config-deepseek',
  providerModelId: 'deepseek-flash',
  displayName: 'Provider Model',
  apiTokenOverrideStatus: 'missing',
  apiTokenOverrideMutation: { type: 'keep' },
  supportsImage: false,
  contextWindowTokens: 128_000,
  providerProfileConfig: {
    schemaVersion: 1,
    profile: { id: 'generic_openai_chat', version: 1 },
    reasoning: { mode: 'provider_default', effort: 'provider_default' }
  },
  inputPrice: '0',
  cachedInputPrice: '',
  outputPrice: '0',
  providerProfileUpdate: { kind: 'unchanged' },
  enabled: true,
  execution: { status: 'available' }
}

const commonProps = {
  globalApiUrl: 'https://provider.example/v1/chat/completions',
  providerProfileDescriptors: profileDescriptors,
  providerVendorDescriptors: vendorDescriptors,
  resolveProviderVendorModelPolicy: vi.fn(resolvePolicy),
  onCancel: vi.fn()
}

describe('ModelForm vendor controls', () => {
  it('switches between provider model choices and free text without replacing the draft', async () => {
    const onSave = vi.fn()
    const screen = await render(<ModelForm {...commonProps} onSave={onSave} />)
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('custom-model')
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('My model')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    const url = screen.getByPlaceholder('configuration.modelApiUrlPlaceholder')
    await url.fill('https://custom.example/v1/chat/completions')
    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.deepSeek' }).click()
    await expect.element(url).toHaveValue('https://api.deepseek.com/chat/completions')
    await expect.element(url).toHaveAttribute('readonly')

    const modelPicker = screen.getByRole('button', {
      name: 'configuration.providerModelId: custom-model'
    })
    await expect.element(modelPicker).toBeVisible()
    await expect
      .element(screen.getByText('configuration.selectSupportedProviderModel'))
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeDisabled()
    await modelPicker.click()
    await expect.element(screen.getByRole('option', { name: 'custom-model' })).toBeDisabled()
    await expect.element(screen.getByRole('option', { name: 'deepseek-flash' })).toBeVisible()
    await expect.element(screen.getByRole('option', { name: 'deepseek-v4-pro' })).toBeVisible()
    expect(screen.getByRole('option').all()).toHaveLength(3)
    await screen.getByRole('option', { name: 'deepseek-flash', exact: true }).click()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeEnabled()

    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.moonshot' }).click()
    await expect.element(url).toHaveValue('https://api.moonshot.cn/v1/chat/completions')
    await expect.element(url).toHaveAttribute('readonly')
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.providerModelId: deepseek-flash'
        })
      )
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeDisabled()
    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.generic' }).click()
    await expect.element(url).not.toHaveAttribute('readonly')
    await expect.element(url).toHaveValue('https://custom.example/v1/chat/completions')
    await expect
      .element(screen.getByPlaceholder('configuration.providerModelIdPlaceholder'))
      .toHaveValue('deepseek-flash')
    await screen
      .getByPlaceholder('configuration.providerModelIdPlaceholder')
      .fill('another-custom-model')
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeEnabled()
    expect(onSave).not.toHaveBeenCalled()
  })

  it.each([
    ['deepSeek', 'deepseek-flash', 'https://api.deepseek.com/chat/completions'],
    ['moonshot', 'kimi-k3.1-pro', 'https://api.moonshot.cn/v1/chat/completions']
  ])('saves the fixed %s URL and a model-specific credential', async (vendor, modelId, apiUrl) => {
    const onSave = vi.fn()
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerModelId: modelId,
          contextWindowTokens: 1_048_576
        }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: `configuration.providerProfile.${vendor}` }).click()
    await expect
      .element(screen.getByPlaceholder('configuration.modelApiUrlPlaceholder'))
      .toHaveValue(apiUrl)
    await expect.element(screen.getByText('configuration.fixedProviderTokenRequired')).toBeVisible()
    await screen.getByLabelText('configuration.modelApiToken').fill('test-only-provider-key')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      providerModelId: modelId,
      apiUrlOverride: apiUrl,
      apiTokenOverrideMutation: { type: 'replace', value: 'test-only-provider-key' }
    })
  })

  it('preserves a matching global connection until a dedicated token is entered', async () => {
    const onSave = vi.fn()
    const screen = await render(
      <ModelForm
        {...commonProps}
        globalApiUrl="https://api.moonshot.cn/v1/chat/completions"
        model={{
          ...model,
          providerModelId: 'kimi-k3',
          contextWindowTokens: 1_048_576,
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'moonshot',
            profile: { id: 'moonshot_k3_chat', version: 1 },
            settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
          }
        }}
        onSave={onSave}
      />
    )
    await expect
      .element(screen.getByPlaceholder('configuration.modelApiUrlPlaceholder'))
      .toHaveValue('https://api.moonshot.cn/v1/chat/completions')
    await expect
      .element(screen.getByPlaceholder('configuration.modelApiUrlPlaceholder'))
      .toHaveAttribute('readonly')
    await expect
      .element(screen.getByText('configuration.fixedProviderTokenRequired'))
      .not.toBeInTheDocument()
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Renamed Kimi')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      displayName: 'Renamed Kimi',
      apiUrlOverride: '',
      apiTokenOverrideMutation: { type: 'keep' },
      providerProfileUpdate: { kind: 'unchanged' }
    })
    await screen.getByLabelText('configuration.modelApiToken').fill('test-only-kimi-key')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(2)
    expect(onSave.mock.calls[1]![0]).toMatchObject({
      apiUrlOverride: 'https://api.moonshot.cn/v1/chat/completions',
      apiTokenOverrideMutation: { type: 'replace', value: 'test-only-kimi-key' }
    })
  })

  it('keeps the fixed URL after clearing a model token instead of inheriting a global token', async () => {
    const onSave = vi.fn()
    const screen = await render(
      <ModelForm
        {...commonProps}
        globalApiUrl="https://api.moonshot.cn/v1/chat/completions"
        model={{
          ...model,
          providerModelId: 'kimi-k3',
          apiTokenOverrideStatus: 'configured',
          apiUrlOverride: 'https://api.moonshot.cn/v1/chat/completions',
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'moonshot',
            profile: { id: 'moonshot_k3_chat', version: 1 },
            settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
          }
        }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.credential.clear' }).click()
    await screen.getByRole('button', { name: 'configuration.credential.clearConfirm' }).click()
    await expect.element(screen.getByText('configuration.fixedProviderTokenRequired')).toBeVisible()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      apiUrlOverride: 'https://api.moonshot.cn/v1/chat/completions',
      apiTokenOverrideMutation: { type: 'clear' }
    })
  })

  it('requires an explicit model choice when an empty draft switches to a fixed provider', async () => {
    const screen = await render(<ModelForm {...commonProps} onSave={vi.fn()} />)
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.moonshot' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerModelId: configuration.selectProviderModel'
      })
      .click()
    await expect
      .element(screen.getByRole('option', { name: 'configuration.selectProviderModel' }))
      .toBeDisabled()
    await expect
      .element(screen.getByRole('option', { name: 'kimi-k3.1-pro', exact: true }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('option', { name: 'kimi-k3.1-pro-highspeed', exact: true }))
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeDisabled()
  })

  it.each(['kimi-k3.1-pro', 'kimi-k3.1-pro-highspeed'])(
    'saves %s through the K3 family while preserving the existing user configuration',
    async (providerModelId) => {
      const onSave = vi.fn()
      const screen = await render(
        <ModelForm
          {...commonProps}
          model={{
            ...model,
            providerModelId: 'kimi-k3',
            contextWindowTokens: 800_000,
            apiUrlOverride: 'https://api.moonshot.cn/v1/chat/completions',
            apiTokenOverrideStatus: 'configured',
            supportsImage: true,
            inputPrice: '0.02',
            cachedInputPrice: '0.002',
            outputPrice: '0.06',
            providerProfileConfig: {
              schemaVersion: 2,
              vendorId: 'moonshot',
              profile: { id: 'moonshot_k3_chat', version: 1 },
              settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
            }
          }}
          onSave={onSave}
        />
      )
      await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeEnabled()
      await screen.getByRole('button', { name: 'configuration.providerModelId: kimi-k3' }).click()
      await screen.getByRole('option', { name: providerModelId, exact: true }).click()
      await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
      await expect
        .element(
          screen.getByRole('button', {
            name: 'configuration.moonshotSettings.reasoningEffort: configuration.moonshotSettings.effortLow'
          })
        )
        .toBeVisible()
      await screen.getByText('configuration.providerSettings.cancel', { exact: true }).click()
      await screen.getByRole('button', { name: 'configuration.save' }).click()
      await expect.poll(() => onSave.mock.calls.length).toBe(1)
      expect(onSave.mock.calls[0]![0]).toMatchObject({
        providerModelId,
        displayName: model.displayName,
        contextWindowTokens: '800000',
        apiUrlOverride: 'https://api.moonshot.cn/v1/chat/completions',
        apiTokenOverrideStatus: 'configured',
        apiTokenOverrideMutation: { type: 'keep' },
        inputPrice: '0.02',
        cachedInputPrice: '0.002',
        outputPrice: '0.06',
        providerProfileUpdate: { kind: 'unchanged' }
      })
    }
  )

  it('keeps an unsupported stored model visible until the user chooses a supported replacement', async () => {
    const onSave = vi.fn()
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerModelId: 'retired-kimi',
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'moonshot',
            profile: { id: 'moonshot_k3_chat', version: 1 },
            settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
          }
        }}
        onSave={onSave}
      />
    )
    await expect
      .element(screen.getByText('configuration.selectSupportedProviderModel'))
      .toBeVisible()
    await screen
      .getByRole('button', { name: 'configuration.providerModelId: retired-kimi' })
      .click()
    await expect.element(screen.getByRole('option', { name: 'retired-kimi' })).toBeDisabled()
    expect(onSave).not.toHaveBeenCalled()
    await screen.getByRole('option', { name: 'kimi-k3.1-pro', exact: true }).click()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeEnabled()
    await expect
      .element(screen.getByText('configuration.selectSupportedProviderModel'))
      .not.toBeInTheDocument()
  })

  it('locates advanced settings without discarding a draft, changing vendors or saving', async () => {
    const onSave = vi.fn()
    const resolver = vi.fn(resolvePolicy)
    const renderForm = (target: SettingsNavigationTarget | null) => (
      <SettingsSearchNavigationProvider target={target}>
        <ModelForm
          {...commonProps}
          model={model}
          resolveProviderVendorModelPolicy={resolver}
          onSave={onSave}
        />
      </SettingsSearchNavigationProvider>
    )
    const screen = await render(renderForm(null))
    await expect.poll(() => resolver.mock.calls.length).toBe(1)
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Unsaved name')

    await screen.rerender(
      renderForm({
        page: 'configuration',
        id: 'configuration.model.apiUrl',
        view: 'model-advanced',
        revision: 1
      })
    )
    await expect
      .element(screen.getByRole('button', { name: 'configuration.more' }))
      .toHaveAttribute('aria-expanded', 'true')
    await expect
      .element(screen.getByPlaceholder('configuration.displayNamePlaceholder'))
      .toHaveValue('Unsaved name')
    expect(document.querySelector('[data-setting-id="configuration.model.apiUrl"]')).not.toBeNull()

    await screen.rerender(
      renderForm({
        page: 'configuration',
        id: 'configuration.model.deepseek.reasoningEffort',
        view: 'model-deepseek',
        revision: 2
      })
    )
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
        })
      )
      .toBeVisible()
    expect(resolver).toHaveBeenCalledTimes(1)
    expect(onSave).not.toHaveBeenCalled()
  })

  it('waits for provider policy before exposing the requested provider field in its existing dialog', async () => {
    const onSave = vi.fn()
    const pendingPolicy = deferred<ProviderVendorModelPolicyDescriptor>()
    const resolver = vi.fn(() => pendingPolicy.promise)
    const configuredModel: ModelConfig = {
      ...model,
      providerProfileConfig: {
        schemaVersion: 2,
        vendorId: 'deepseek',
        profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
        settings: {
          kind: 'deepseek_flash_chat',
          reasoning: { mode: 'enabled', effort: 'high' }
        }
      }
    }
    const screen = await render(
      <SettingsSearchNavigationProvider
        target={{
          page: 'configuration',
          id: 'configuration.model.deepseek.reasoningEffort',
          view: 'model-deepseek',
          revision: 1
        }}
      >
        <ModelForm
          {...commonProps}
          model={configuredModel}
          resolveProviderVendorModelPolicy={resolver}
          onSave={onSave}
        />
      </SettingsSearchNavigationProvider>
    )
    await expect.poll(() => resolver.mock.calls.length).toBe(1)
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    pendingPolicy.resolve(
      await resolvePolicy({
        vendorId: 'deepseek',
        modelId: configuredModel.providerModelId,
        dialect: 'openai_chat_completions'
      })
    )
    await expect
      .element(screen.getByRole('dialog', { name: 'configuration.deepSeekSettings.title' }))
      .toBeVisible()
    expect(
      document.querySelector('[data-setting-id="configuration.model.deepseek.reasoningEffort"]')
    ).not.toBeNull()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortHigh'
        })
      )
      .toBeVisible()
    expect(onSave).not.toHaveBeenCalled()
  })

  it('resolves provider policy from the provider model ID, not the editable display name', async () => {
    const resolver = vi.fn(resolvePolicy)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={model}
        resolveProviderVendorModelPolicy={resolver}
        onSave={vi.fn()}
      />
    )

    await expect.poll(() => resolver.mock.calls.length).toBe(1)
    expect(resolver.mock.calls[0]![0]).toMatchObject({ modelId: 'deepseek-flash' })

    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Friendly alias')
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    expect(resolver).toHaveBeenCalledTimes(1)

    await screen
      .getByPlaceholder('configuration.providerModelIdPlaceholder')
      .fill('deepseek-v4-pro')
    await expect.poll(() => resolver.mock.calls.length).toBe(2)
    expect(resolver.mock.calls[1]![0]).toMatchObject({ modelId: 'deepseek-v4-pro' })
  })

  it('requires a display name and enforces the 512-byte downstream snapshot limit', async () => {
    const onSave = vi.fn()
    const screen = await render(<ModelForm {...commonProps} onSave={onSave} />)
    const displayNameInput = screen.getByPlaceholder('configuration.displayNamePlaceholder')
    const saveButton = screen.getByRole('button', { name: 'configuration.save' })

    await expect.element(displayNameInput).toHaveAttribute('required')
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('generic-model')
    await displayNameInput.fill('😀'.repeat(129))
    await expect.element(screen.getByText('configuration.invalidDisplayName')).toBeVisible()
    await expect.element(saveButton).toBeDisabled()

    await displayNameInput.fill('😀'.repeat(128))
    await expect
      .element(screen.getByText('configuration.invalidDisplayName'))
      .not.toBeInTheDocument()
    await expect.poll(() => (saveButton.element() as HTMLButtonElement).disabled).toBe(false)
    await saveButton.click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].displayName).toBe('😀'.repeat(128))
  })

  it('allows an override URL to be saved before its credential is configured', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(<ModelForm {...commonProps} onSave={onSave} />)
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('generic-model')
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Generic model')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByPlaceholder('configuration.modelApiUrlPlaceholder')
      .fill('https://override.example/v1/chat/completions')

    await expect
      .element(screen.getByText('configuration.modelConnectionPairRequired'))
      .toBeVisible()
    const saveButton = screen.getByRole('button', { name: 'configuration.save' })
    await expect.poll(() => (saveButton.element() as HTMLButtonElement).disabled).toBe(false)
    await saveButton.click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      apiUrlOverride: 'https://override.example/v1/chat/completions',
      apiTokenOverrideMutation: { type: 'keep' },
      apiTokenOverrideStatus: 'missing'
    })
  })

  it('rejects insecure or credential-bearing override URLs before Host submission', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(<ModelForm {...commonProps} onSave={onSave} />)
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('generic-model')
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Generic model')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    const url = screen.getByPlaceholder('configuration.modelApiUrlPlaceholder')
    const saveButton = screen.getByRole('button', { name: 'configuration.save' })

    await url.fill('http://provider.example/v1/chat/completions')
    await expect.element(screen.getByText('configuration.invalidApiUrl')).toBeVisible()
    await expect.element(saveButton).toBeDisabled()

    await url.fill('https://user@provider.example/v1/chat/completions?token=secret')
    await expect.element(screen.getByText('configuration.invalidApiUrl')).toBeVisible()
    await expect.element(saveButton).toBeDisabled()

    await url.fill('https://provider.example/v1/chat/completions')
    await expect.element(screen.getByText('configuration.invalidApiUrl')).not.toBeInTheDocument()
    await expect.poll(() => (saveButton.element() as HTMLButtonElement).disabled).toBe(false)
  })

  it('allows an override credential to be saved before its URL is configured', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(<ModelForm {...commonProps} onSave={onSave} />)
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('generic-model')
    await screen.getByPlaceholder('configuration.displayNamePlaceholder').fill('Generic model')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen.getByLabelText('configuration.modelApiToken').fill('override-key')

    await expect
      .element(screen.getByText('configuration.modelConnectionPairRequired'))
      .toBeVisible()
    const saveButton = screen.getByRole('button', { name: 'configuration.save' })
    await expect.poll(() => (saveButton.element() as HTMLButtonElement).disabled).toBe(false)
    await saveButton.click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      apiUrlOverride: '',
      apiTokenOverrideMutation: { type: 'replace', value: 'override-key' }
    })
  })

  it('allows a configured override credential to be cleared while preserving its URL', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          apiUrlOverride: 'https://override.example/v1/chat/completions',
          apiTokenOverrideStatus: 'configured'
        }}
        onSave={onSave}
      />
    )

    await screen.getByRole('button', { name: 'configuration.credential.clear' }).click()
    await screen.getByRole('button', { name: 'configuration.credential.clearConfirm' }).click()
    await expect
      .element(screen.getByText('configuration.modelConnectionPairRequired'))
      .toBeVisible()
    const saveButton = screen.getByRole('button', { name: 'configuration.save' })
    await expect.poll(() => (saveButton.element() as HTMLButtonElement).disabled).toBe(false)
    await saveButton.click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      apiUrlOverride: 'https://override.example/v1/chat/completions',
      apiTokenOverrideMutation: { type: 'clear' },
      apiTokenOverrideStatus: 'configured'
    })
  })

  it('shows only localized vendor names and never projects family/profile identities', async () => {
    const screen = await render(<ModelForm {...commonProps} model={model} onSave={vi.fn()} />)
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    const labels = Array.from(
      document.querySelectorAll<HTMLElement>(
        '.settings-select__menu .settings-select__option-label'
      )
    ).map((option) => option.textContent)
    expect(labels).toEqual([
      'configuration.providerProfile.generic',
      'configuration.providerProfile.deepSeek',
      'configuration.providerProfile.moonshot'
    ])
    expect(document.body.textContent).not.toMatch(/V4 Chat|Kimi K3|deepseek_v4|moonshot_k3|@1/)
  })

  it('keeps an unknown stored profile opaque while allowing an unchanged round-trip', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'future_private_profile', version: 7 },
            reasoning: { mode: 'provider_default', effort: 'provider_default' }
          }
        }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    expect(document.body.textContent).toContain('configuration.providerProfile.unsupported')
    expect(document.body.textContent).not.toContain('future_private_profile')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({ kind: 'unchanged' })
  })

  it('keeps a future version of a known V2 family opaque without downgrading it', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerModelId: 'kimi-k3',
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'moonshot',
            profile: { id: 'moonshot_k3_chat', version: 99 },
            settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
          }
        }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    expect(document.body.textContent).toContain('configuration.providerProfile.unsupported')
    expect(document.body.textContent).not.toMatch(/moonshot_k3_chat|@99/)
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({ kind: 'unchanged' })
  })

  it('submits DeepSeek Low through the vendor-owned V2 settings union', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(<ModelForm {...commonProps} model={model} onSave={onSave} />)
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.deepSeek' }).click()
    await expect
      .poll(
        () =>
          (
            screen
              .getByRole('button', { name: 'configuration.providerSettings.open' })
              .element() as HTMLButtonElement
          ).disabled
      )
      .toBe(false)
    const imageInput = screen.getByRole('switch', { name: 'configuration.supportsImage' })
    await expect.element(imageInput).toHaveAttribute('aria-checked', 'true')
    await expect.element(imageInput).toBeDisabled()
    expect(document.querySelector('.model-form-capability-note')).toBeNull()
    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortProviderDefault'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.deepSeekSettings.effortLow' }).click()
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({
      kind: 'select_vendor',
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'provider_default', effort: 'low' }
      }
    })
  })

  it('uses the Pro descriptor, disables image input, and restores settings after cancel', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerModelId: 'deepseek-v4-pro',
          supportsImage: true,
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'deepseek',
            profile: { id: 'deepseek_v4_pro_0813_chat', version: 1 },
            settings: {
              kind: 'deepseek_pro_chat',
              reasoning: { mode: 'enabled', effort: 'high' }
            }
          }
        }}
        onSave={onSave}
      />
    )

    const imageInput = screen.getByRole('switch', { name: 'configuration.supportsImage' })
    await expect.element(imageInput).toHaveAttribute('aria-checked', 'false')
    await expect.element(imageInput).toBeDisabled()
    await expect
      .poll(
        () =>
          (
            screen
              .getByRole('button', { name: 'configuration.providerSettings.open' })
              .element() as HTMLButtonElement
          ).disabled
      )
      .toBe(false)
    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingEnabled'
        })
      )
      .toBeVisible()
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortHigh'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.deepSeekSettings.effortMax' }).click()
    await screen.getByText('configuration.providerSettings.cancel').click()

    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.deepSeekSettings.thinkingMode: configuration.deepSeekSettings.thinkingEnabled'
        })
      )
      .toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortHigh'
        })
      )
      .toBeVisible()
    expect(onSave).not.toHaveBeenCalled()
  })

  it('keeps save disabled while resolving and ignores an older response that finishes last', async () => {
    const older = deferred<ProviderVendorModelPolicyDescriptor>()
    const newer = deferred<ProviderVendorModelPolicyDescriptor>()
    let olderDelivered = false
    const resolver = vi.fn((input: ProviderVendorModelPolicyInput) => {
      if (input.modelId !== 'older-model') return newer.promise
      return older.promise.then((policy) => {
        olderDelivered = true
        return policy
      })
    })
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{ ...model, providerModelId: 'older-model' }}
        resolveProviderVendorModelPolicy={resolver}
        onSave={vi.fn()}
      />
    )
    const save = screen.getByRole('button', { name: 'configuration.save' })

    await expect.poll(() => resolver.mock.calls.length).toBe(1)
    await expect.element(save).toBeDisabled()
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('newer-model')
    await expect.poll(() => resolver.mock.calls.length).toBe(2)
    await expect.element(save).toBeDisabled()

    newer.resolve({
      status: 'unsupported',
      vendorId: 'generic',
      reason: 'unsupported_model'
    })
    await expect
      .element(screen.getByText('configuration.providerProfile.unsupportedModel'))
      .toBeVisible()
    await expect.element(save).toBeDisabled()

    older.resolve(
      genericPolicy({
        vendorId: 'generic',
        modelId: 'older-model',
        dialect: 'openai_chat_completions'
      })
    )
    await expect.poll(() => olderDelivered).toBe(true)
    await new Promise((resolve) => window.setTimeout(resolve, 0))
    await expect
      .element(screen.getByText('configuration.providerProfile.unsupportedModel'))
      .toBeVisible()
    await expect.element(save).toBeDisabled()
  })

  it('recovers after a resolver rejection only after a later model resolves successfully', async () => {
    const resolver = vi.fn((input: ProviderVendorModelPolicyInput) =>
      input.modelId === 'rejected-model'
        ? Promise.reject(new Error('resolver unavailable'))
        : Promise.resolve(genericPolicy(input))
    )
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{ ...model, providerModelId: 'rejected-model' }}
        resolveProviderVendorModelPolicy={resolver}
        onSave={vi.fn()}
      />
    )
    const save = screen.getByRole('button', { name: 'configuration.save' })

    await expect
      .element(screen.getByText('configuration.providerProfile.resolveFailed'))
      .toBeVisible()
    await expect.element(save).toBeDisabled()

    await screen
      .getByPlaceholder('configuration.providerModelIdPlaceholder')
      .fill('recovered-model')
    await expect.poll(() => resolver.mock.calls.length).toBe(2)
    await expect
      .element(screen.getByText('configuration.providerProfile.resolveFailed'))
      .not.toBeInTheDocument()
    await expect.element(save).toBeEnabled()
  })

  it('uses the K3 descriptor, enables image input, and saves Moonshot settings explicitly', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{ ...model, providerModelId: 'kimi-k3' }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.moonshot' }).click()
    await expect
      .poll(
        () =>
          (
            screen
              .getByRole('button', { name: 'configuration.providerSettings.open' })
              .element() as HTMLButtonElement
          ).disabled
      )
      .toBe(false)
    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    expect(document.body.textContent).toContain(
      'configuration.moonshotSettings.alwaysPreservedThinking'
    )
    expect(document.body.textContent).not.toContain('configuration.moonshotSettings.thinkingMode')
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      supportsImage: true,
      providerProfileUpdate: {
        kind: 'select_vendor',
        vendorId: 'moonshot',
        settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
      }
    })
  })

  it('resets settings and warns when the Host resolves a different Moonshot family', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const k3Model: ModelConfig = {
      ...model,
      providerModelId: 'kimi-k3',
      supportsImage: true,
      providerProfileConfig: {
        schemaVersion: 2,
        vendorId: 'moonshot',
        profile: { id: 'moonshot_k3_chat', version: 1 },
        settings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
      }
    }
    const screen = await render(<ModelForm {...commonProps} model={k3Model} onSave={onSave} />)
    await screen.getByRole('button', { name: 'configuration.providerModelId: kimi-k3' }).click()
    await screen.getByRole('option', { name: 'kimi-k2.6', exact: true }).click()
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await expect
      .element(screen.getByText('configuration.providerProfile.familyChanged'))
      .toBeVisible()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({
      kind: 'select_vendor',
      vendorId: 'moonshot',
      settings: { kind: 'moonshot_k2_6_chat', thinkingMode: 'provider_default' }
    })
  })

  it('requires resolving a stored model mismatch before saving its fixed provider URL', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        globalApiUrl="https://proxy.example/v1/chat/completions"
        model={{
          ...model,
          providerModelId: 'kimi-k3',
          providerProfileConfig: {
            schemaVersion: 2,
            vendorId: 'deepseek',
            profile: { id: 'deepseek_v4_1_flash_chat', version: 1 },
            settings: {
              kind: 'deepseek_flash_chat',
              reasoning: { mode: 'enabled', effort: 'high' }
            }
          }
        }}
        onSave={onSave}
      />
    )
    await expect
      .element(screen.getByText('configuration.providerProfile.unsupportedModel'))
      .toBeVisible()
    expect(
      screen.getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.deepSeek'
      })
    ).toBeTruthy()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeDisabled()
    await expect
      .element(screen.getByPlaceholder('configuration.modelApiUrlPlaceholder'))
      .toHaveValue('https://api.deepseek.com/chat/completions')
    expect(onSave).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: /^configuration.providerProfile.vendor:/ }).click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.generic' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0]).toMatchObject({
      providerModelId: 'kimi-k3',
      apiUrlOverride: '',
      providerProfileUpdate: { kind: 'select_generic' }
    })
  })

  it('blocks an unknown Moonshot model and keeps the Host save rejection sanitized', async () => {
    const onSave = vi.fn().mockRejectedValue(new Error('raw provider payload'))
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{ ...model, providerModelId: 'future-kimi' }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.moonshot' }).click()
    await expect
      .element(screen.getByText('configuration.providerProfile.unsupportedModel'))
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'configuration.save' })).toBeDisabled()

    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.moonshot'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.generic' }).click()
    await expect
      .poll(
        () =>
          (
            screen
              .getByRole('button', { name: 'configuration.save' })
              .element() as HTMLButtonElement
          ).disabled
      )
      .toBe(false)
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('configuration.saveFailedSafe')
    expect(document.body.textContent).not.toContain('raw provider payload')
  })

  it('keeps the complete draft and focuses the context window after acknowledging a capacity rejection', async () => {
    const onSave = vi.fn().mockRejectedValueOnce(
      new HostInvocationError({
        message: 'private capacity diagnostic',
        code: -32000,
        data: {
          kind: 'model_settings_validation',
          code: 'invalid_context_capacity_configuration',
          modelId: model.id,
          displayName: 'Draft Max',
          contextWindowTokens: 128000,
          reservedOutputTokens: 131072,
          safetyMarginTokens: 6400,
          minimumContextWindowTokens: 137972
        }
      })
    )
    const onCancel = vi.fn()
    const screen = await render(
      <ModelForm {...commonProps} model={model} onSave={onSave} onCancel={onCancel} />
    )
    const nameInput = screen.getByPlaceholder('configuration.displayNamePlaceholder')
    const contextInput = screen.getByPlaceholder('configuration.contextWindowTokensPlaceholder')
    await nameInput.fill('Draft Max')
    await contextInput.fill('128000')
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect
      .element(screen.getByRole('alertdialog'))
      .toHaveTextContent('configuration.invalidContextCapacity:Draft Max:128000:131072:6400')
    const acknowledge = document.querySelector<HTMLButtonElement>(
      '.app-confirm-dialog__button--primary'
    )!
    expect(acknowledge.textContent).toBe('知道了')
    expect(document.querySelector('.app-confirm-dialog__button--cancel')).toBeNull()
    await expect.poll(() => document.activeElement).toBe(acknowledge)
    expect(document.body.textContent).not.toContain('private capacity diagnostic')
    expect(document.body.textContent).not.toContain('invalid_context_capacity_configuration')

    acknowledge.click()
    await expect.element(screen.getByRole('alertdialog')).not.toBeInTheDocument()
    await expect.element(contextInput).toHaveFocus()
    await expect.element(contextInput).toHaveValue('128000')
    await expect.element(nameInput).toHaveValue('Draft Max')
    expect(onCancel).not.toHaveBeenCalled()
    expect(onSave).toHaveBeenCalledTimes(1)

    await contextInput.fill('256000')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(2)
    expect(onSave.mock.calls[1]![0]).toMatchObject({
      displayName: 'Draft Max',
      contextWindowTokens: '256000'
    })
  })

  it('keeps the complete draft and focuses display name after the duplicate dialog closes', async () => {
    const onSave = vi
      .fn()
      .mockRejectedValueOnce(
        new HostInvocationError({
          message: 'Model settings validation failed.',
          code: -32000,
          data: {
            kind: 'model_settings_validation',
            code: 'duplicate_display_name',
            displayName: 'Conflicting display'
          }
        })
      )
      .mockResolvedValueOnce(undefined)
    const screen = await render(<ModelForm {...commonProps} model={model} onSave={onSave} />)
    const displayNameInput = screen.getByPlaceholder('configuration.displayNamePlaceholder')

    await displayNameInput.fill('Conflicting display')
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.generic'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.providerProfile.deepSeek' }).click()
    await expect
      .poll(
        () =>
          (
            screen
              .getByRole('button', { name: 'configuration.save' })
              .element() as HTMLButtonElement
          ).disabled
      )
      .toBe(false)
    await screen.getByRole('button', { name: 'configuration.providerSettings.open' }).click()
    await screen
      .getByRole('button', {
        name: 'configuration.deepSeekSettings.reasoningEffort: configuration.deepSeekSettings.effortProviderDefault'
      })
      .click()
    await screen.getByRole('option', { name: 'configuration.deepSeekSettings.effortLow' }).click()
    await screen.getByRole('button', { name: 'configuration.providerSettings.confirm' }).click()
    await screen.getByRole('button', { name: 'configuration.save' }).click()

    await expect
      .element(screen.getByRole('alertdialog'))
      .toHaveTextContent('configuration.duplicateDisplayName:Conflicting display')
    await expect
      .element(screen.getByRole('heading', { name: 'configuration.duplicateDisplayNameTitle' }))
      .toBeVisible()
    const acknowledge = document.querySelector<HTMLButtonElement>(
      '.app-confirm-dialog__button--primary'
    )!
    await expect.poll(() => document.activeElement).toBe(acknowledge)
    expect(document.body.textContent).not.toContain('Model settings validation failed.')

    acknowledge.click()
    await expect.element(screen.getByRole('alertdialog')).not.toBeInTheDocument()
    await expect.element(displayNameInput).toHaveFocus()
    await expect.element(displayNameInput).toHaveValue('Conflicting display')
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({
      kind: 'select_vendor',
      vendorId: 'deepseek',
      settings: {
        kind: 'deepseek_flash_chat',
        reasoning: { mode: 'provider_default', effort: 'low' }
      }
    })

    await displayNameInput.fill('Available display')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(2)
    expect(onSave.mock.calls[1]![0]).toMatchObject({
      displayName: 'Available display',
      providerModelId: 'deepseek-flash'
    })
  })
})
