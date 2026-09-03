import { describe, expect, it } from 'vitest'
import {
  parseProviderProfileUiDescriptors,
  parseProviderVendorDescriptors,
  parseProviderVendorModelPolicyDescriptor,
  parseStorageForkConversationErrorData,
  parseStorageForkConversationRequest,
  parseStorageModelSettingsRecord,
  parseStorageModelSettingsUpdateRecord,
  parseStorageModelSettingsValidationErrorData
} from './storageParsers'

describe('Provider Profile UI descriptor parser', () => {
  const descriptors = [
    {
      profileId: 'deepseek_v4_chat',
      profileVersion: 1,
      displayName: 'DeepSeek',
      compatibleDialects: ['openai_chat_completions'],
      settingsKind: 'deepseek_v4_chat',
      selectable: true
    }
  ] as const

  it('accepts only the bounded presentation-safe projection', () => {
    expect(parseProviderProfileUiDescriptors(descriptors)).toEqual(descriptors)
  })

  it.each([
    [{ ...descriptors[0], credentialRef: 'private-reference' }],
    [{ ...descriptors[0], runtimeCapabilities: ['private'] }],
    [{ ...descriptors[0], compatibleDialects: ['openai_chat_completions', 'unknown'] }],
    [{ ...descriptors[0], displayName: 'x'.repeat(257) }],
    [descriptors[0], descriptors[0]]
  ])('rejects secret, runtime, malformed, oversized, and duplicate projections %#', (value) => {
    expect(() => parseProviderProfileUiDescriptors(value)).toThrow(
      'Invalid Provider Profile UI descriptors'
    )
  })
})

describe('Provider vendor descriptor parsers', () => {
  const vendors = [
    { vendorId: 'generic', displayName: 'Generic', selectable: true },
    { vendorId: 'deepseek', displayName: 'DeepSeek', selectable: true },
    { vendorId: 'moonshot', displayName: 'Moonshot', selectable: true }
  ] as const

  it('accepts the bounded public vendor directory and exact K3 policy union', () => {
    expect(parseProviderVendorDescriptors(vendors)).toEqual(vendors)
    const policy = {
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
    } as const
    expect(parseProviderVendorModelPolicyDescriptor(policy)).toEqual(policy)
  })

  it('keeps future bounded vendors opaque instead of rejecting the whole directory', () => {
    const future = { vendorId: 'future_vendor.v1', displayName: 'Future', selectable: true }
    expect(parseProviderVendorDescriptors([...vendors, future])).toEqual([...vendors, future])
    expect(
      parseProviderVendorModelPolicyDescriptor({
        status: 'unsupported',
        vendorId: future.vendorId,
        reason: 'unsupported_vendor'
      })
    ).toEqual({
      status: 'unsupported',
      vendorId: future.vendorId,
      reason: 'unsupported_vendor'
    })
  })

  it.each([
    [...vendors, { vendorId: 'moonshot', displayName: 'Duplicate', selectable: true }],
    [{ vendorId: 'Future Vendor', displayName: 'Future', selectable: true }],
    [{ vendorId: 'a'.repeat(33), displayName: 'Future', selectable: true }],
    [{ vendorId: 'moonshot', displayName: 'Moonshot', selectable: true, profileId: 'private' }]
  ])('rejects malformed or expanded vendor descriptors %#', (value) => {
    expect(() => parseProviderVendorDescriptors(value)).toThrow(
      'Invalid Provider vendor descriptors'
    )
  })

  it.each([
    {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k3_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      profileVersion: 1,
      settings: {
        kind: 'moonshot_k3_chat',
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'max' }
      }
    },
    {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'deepseek_v4_chat',
      settingsKind: 'deepseek',
      imageInput: 'supported',
      settings: {
        kind: 'deepseek_v4_chat',
        reasoningModes: ['provider_default', 'enabled', 'disabled'],
        reasoningEfforts: ['provider_default', 'low', 'high', 'max'],
        defaultSettings: {
          kind: 'deepseek_v4_chat',
          reasoning: { mode: 'provider_default', effort: 'provider_default' }
        }
      }
    },
    {
      status: 'supported',
      vendorId: 'moonshot',
      modelFamily: 'moonshot_k3_chat',
      settingsKind: 'moonshot',
      imageInput: 'supported',
      settings: {
        kind: 'moonshot_k3_chat',
        reasoningEfforts: ['provider_default', 'high', 'max'],
        defaultSettings: { kind: 'moonshot_k3_chat', reasoningEffort: 'low' }
      }
    }
  ])('rejects private, cross-family, or internally inconsistent policy fields %#', (value) => {
    expect(() => parseProviderVendorModelPolicyDescriptor(value)).toThrow(
      'Invalid Provider vendor model policy descriptor'
    )
  })
})

const valid = {
  type: 'conversation_fork',
  code: 'active_command_session',
  conversationId: 'conversation-1',
  activeSessionCount: 1
} as const

describe('storage protocol parsers', () => {
  it.each([
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' }
    },
    {
      requestId: 'request-2',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'provider_transition_boundary', operationId: 'operation-1' }
    }
  ] as const)('parses an explicit timeline fork point %#', (request) => {
    expect(parseStorageForkConversationRequest(request)).toEqual(request)
  })

  it.each([
    null,
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1', operationId: 'x' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'provider_transition_boundary', operationId: '' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'unknown', operationId: 'operation-1' }
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      throughAssistantMessageId: 'assistant-1'
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1'
    },
    {
      requestId: 'request-1',
      sourceConversationId: 'conversation-1',
      forkPoint: { kind: 'assistant_reply', assistantMessageId: 'assistant-1' },
      extra: true
    }
  ])('rejects an ambiguous or malformed timeline fork point %#', (request) => {
    expect(() => parseStorageForkConversationRequest(request)).toThrow(
      'Invalid storage fork conversation request'
    )
  })

  it('parses the bounded active-command fork rejection', () => {
    expect(parseStorageForkConversationErrorData(valid)).toEqual(valid)
    const maximumWidthId = '😀'.repeat(512)
    expect(
      parseStorageForkConversationErrorData({ ...valid, conversationId: maximumWidthId })
    ).toEqual({ ...valid, conversationId: maximumWidthId })
  })

  it.each([
    null,
    { ...valid, type: 'database_error' },
    { ...valid, code: 'unknown' },
    { ...valid, conversationId: '' },
    { ...valid, conversationId: '😀'.repeat(513) },
    { ...valid, activeSessionCount: 0 },
    { ...valid, activeSessionCount: 513 },
    { ...valid, sql: 'private schema detail' }
  ])('rejects malformed or oversized recovery data %#', (value) => {
    expect(() => parseStorageForkConversationErrorData(value)).toThrow(
      'Invalid storage fork conversation error data'
    )
  })
})

describe('model settings validation error parser', () => {
  const duplicate = {
    kind: 'model_settings_validation',
    code: 'duplicate_display_name',
    displayName: 'DeepSeek V4 Flash'
  } as const

  it('accepts only the bounded duplicate-name recovery contract', () => {
    expect(parseStorageModelSettingsValidationErrorData(duplicate)).toEqual(duplicate)
    const maximumWidthName = '😀'.repeat(128)
    expect(
      parseStorageModelSettingsValidationErrorData({ ...duplicate, displayName: maximumWidthName })
    ).toEqual({ ...duplicate, displayName: maximumWidthName })
  })

  it.each([
    null,
    { ...duplicate, kind: 'database_error' },
    { ...duplicate, code: 'invalid_price' },
    { ...duplicate, displayName: '' },
    { ...duplicate, displayName: '😀'.repeat(129) },
    { ...duplicate, legacyIdentity: 'retired-identity' },
    { ...duplicate, sql: 'private schema detail' }
  ])('rejects malformed, oversized, or expanded error data %#', (value) => {
    expect(() => parseStorageModelSettingsValidationErrorData(value)).toThrow(
      'Invalid storage model settings validation error data'
    )
  })
})

describe('secret-free model settings parsers', () => {
  const model = {
    id: 'model-1',
    providerModelId: 'provider-model',
    displayName: 'Model',
    apiUrlOverride: null,
    apiTokenOverrideStatus: 'missing',
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
    enabled: true
  } as const
  const snapshot = {
    configurationRevision: 'model-settings-v1:00000000-0000-4000-8000-000000000001',
    apiUrl: 'https://provider.example/v1',
    apiTokenStatus: 'configured',
    searchMode: 'auto',
    tavilyApiKeyStatus: 'missing',
    models: [model]
  } as const
  const update = {
    expectedRevision: snapshot.configurationRevision,
    apiUrl: snapshot.apiUrl,
    apiTokenMutation: { type: 'keep' },
    searchMode: snapshot.searchMode,
    tavilyApiKeyMutation: { type: 'replace', value: 'new-search-key' },
    models: [
      {
        id: model.id,
        providerModelId: model.providerModelId,
        displayName: model.displayName,
        apiUrlOverride: model.apiUrlOverride,
        apiTokenOverrideMutation: { type: 'clear' },
        supportsImage: model.supportsImage,
        contextWindowTokens: model.contextWindowTokens,
        providerProfileUpdate: { kind: 'unchanged' },
        inputPrice: model.inputPrice,
        cachedInputPrice: model.cachedInputPrice,
        outputPrice: model.outputPrice,
        enabled: model.enabled
      }
    ]
  } as const

  it('accepts only status projections and explicit mutation unions', () => {
    expect(parseStorageModelSettingsRecord(snapshot)).toEqual(snapshot)
    expect(parseStorageModelSettingsUpdateRecord(update)).toEqual(update)
    expect(parseStorageModelSettingsUpdateRecord({ ...update, expectedRevision: null })).toEqual({
      ...update,
      expectedRevision: null
    })
  })

  it.each([
    { ...snapshot, apiToken: 'legacy-secret' },
    { ...snapshot, tavilyApiKey: 'legacy-secret' },
    {
      ...snapshot,
      models: [{ ...model, apiTokenOverride: 'legacy-secret' }]
    },
    {
      ...snapshot,
      models: [{ ...model, credentialRef: 'private-reference' }]
    },
    {
      ...snapshot,
      models: [
        {
          ...model,
          providerProfileConfig: {
            ...model.providerProfileConfig,
            privateRuntimeCapability: true
          }
        }
      ]
    }
  ])('rejects legacy secrets and private references in a load projection %#', (value) => {
    expect(() => parseStorageModelSettingsRecord(value)).toThrow(
      /credential field|unexpected field/
    )
  })

  it('rejects a malformed Host model-settings revision', () => {
    expect(() =>
      parseStorageModelSettingsRecord({
        ...snapshot,
        configurationRevision: 'not-a-model-settings-revision'
      })
    ).toThrow(/configurationRevision/)
  })

  it.each([
    { ...update, expectedRevision: 'not-a-model-settings-revision' },
    { ...update, apiTokenMutation: { type: 'replace' } },
    { ...update, apiTokenMutation: { type: 'replace', value: '' } },
    { ...update, apiTokenMutation: { type: 'keep', value: 'smuggled-secret' } },
    { ...update, tavilyApiKeyMutation: { type: 'unknown' } },
    { ...update, apiTokenMutation: { type: 'replace', value: 'contains whitespace' } },
    {
      ...update,
      models: [{ ...update.models[0], apiTokenOverrideMutation: { type: 'clear', value: 'x' } }]
    },
    {
      ...update,
      models: [
        {
          ...update.models[0],
          providerProfileUpdate: { kind: 'unchanged', profileVersion: 1 }
        }
      ]
    },
    { ...update, apiToken: 'legacy-secret' }
  ])('rejects malformed mutations and legacy update fields %#', (value) => {
    expect(() => parseStorageModelSettingsUpdateRecord(value)).toThrow(
      /storage model settings update request/
    )
  })
})
