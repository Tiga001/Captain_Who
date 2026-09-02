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

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) =>
      key === 'configuration.duplicateDisplayName' ? `${key}:{displayName}` : key
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
    profileId: 'deepseek_v4_chat',
    profileVersion: 1,
    displayName: 'DeepSeek V4 Chat',
    compatibleDialects: ['openai_chat_completions'],
    settingsKind: 'deepseek_v4_chat',
    selectable: true
  },
  {
    profileId: 'deepseek_v4_vision',
    profileVersion: 1,
    displayName: 'DeepSeek Vision',
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
  { vendorId: 'generic', displayName: 'Generic internal label', selectable: true },
  { vendorId: 'deepseek', displayName: 'DeepSeek internal label', selectable: true },
  { vendorId: 'moonshot', displayName: 'Moonshot internal label', selectable: true },
  { vendorId: 'future_vendor.v1', displayName: 'Future internal label', selectable: true }
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
  if (input.vendorId === 'deepseek' && input.modelId === 'deepseek-v4-flash') {
    return Promise.resolve({
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
  }
  if (input.vendorId === 'moonshot' && input.modelId === 'kimi-k3') {
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
  providerModelId: 'deepseek-v4-flash',
  displayName: 'Provider Model',
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
  enabled: true
}

const commonProps = {
  globalApiUrl: 'https://provider.example/v1/chat/completions',
  providerProfileDescriptors: profileDescriptors,
  providerVendorDescriptors: vendorDescriptors,
  resolveProviderVendorModelPolicy: vi.fn(resolvePolicy),
  onCancel: vi.fn()
}

describe('ModelForm vendor controls', () => {
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
    expect(resolver.mock.calls[0]![0]).toMatchObject({ modelId: 'deepseek-v4-flash' })

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
        '.model-provider-profile-select .settings-select__option-label'
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
        kind: 'deepseek_v4_chat',
        reasoning: { mode: 'provider_default', effort: 'low' }
      }
    })
  })

  it('restores legacy DeepSeek settings after cancelling and reopening the dialog', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        model={{
          ...model,
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'deepseek_v4_chat', version: 1 },
            reasoning: { mode: 'enabled', effort: 'high' }
          }
        }}
        onSave={onSave}
      />
    )

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
    await screen.getByPlaceholder('configuration.providerModelIdPlaceholder').fill('kimi-k2.6')
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

  it('does not auto-migrate a Kimi alias stored as DeepSeek on a custom endpoint', async () => {
    const onSave = vi.fn().mockResolvedValue(undefined)
    const screen = await render(
      <ModelForm
        {...commonProps}
        globalApiUrl="https://proxy.example/v1/chat/completions"
        model={{
          ...model,
          providerModelId: 'kimi-k3',
          providerProfileConfig: {
            schemaVersion: 1,
            profile: { id: 'deepseek_v4_chat', version: 1 },
            reasoning: { mode: 'enabled', effort: 'high' }
          }
        }}
        onSave={onSave}
      />
    )
    await screen.getByRole('button', { name: 'configuration.more' }).click()
    await expect
      .element(screen.getByText('configuration.providerProfile.unsupportedModel'))
      .toBeVisible()
    expect(
      screen.getByRole('button', {
        name: 'configuration.providerProfile.vendor: configuration.providerProfile.deepSeek'
      })
    ).toBeTruthy()
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(1)
    expect(onSave.mock.calls[0]![0].providerProfileUpdate).toEqual({ kind: 'unchanged' })
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
        kind: 'deepseek_v4_chat',
        reasoning: { mode: 'provider_default', effort: 'low' }
      }
    })

    await displayNameInput.fill('Available display')
    await screen.getByRole('button', { name: 'configuration.save' }).click()
    await expect.poll(() => onSave.mock.calls.length).toBe(2)
    expect(onSave.mock.calls[1]![0]).toMatchObject({
      displayName: 'Available display',
      providerModelId: 'deepseek-v4-flash'
    })
  })
})
