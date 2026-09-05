import type {
  CredentialMutation,
  CredentialStatus,
  ProviderFamilySettings,
  ProviderFamilySettingsDescriptor,
  ProviderProfileConfig,
  ProviderModelFamilyId,
  ProviderProfileUiDescriptor,
  ProviderReasoningEffort,
  ProviderReasoningMode,
  ProviderVendorDescriptor,
  ProviderVendorId,
  ProviderVendorModelPolicyDescriptor,
  StorageConversationForkPoint,
  StorageForkConversationErrorData,
  StorageForkConversationRequest,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageProviderProfileUpdate,
  StorageModelSettingsValidationErrorData
} from './storage'
import {
  expectArray,
  expectBoolean,
  expectEnum,
  expectNonEmptyString,
  expectOnlyKeys,
  expectRecord,
  expectSafeInteger,
  expectString,
  invalidProtocolValue
} from './skills/validation'

// Core accepts at most 512 Unicode scalar values; four bytes each covers the same identifier set.
const MAX_CONVERSATION_ID_BYTES = 512 * 4
const MAX_FORK_IDENTIFIER_BYTES = 512 * 4
const MAX_ACTIVE_COMMAND_SESSIONS = 512
const MAX_MODEL_DISPLAY_NAME_BYTES = 512
const MAX_MODEL_SETTINGS_REVISION_BYTES = 128
const MAX_PROVIDER_DESCRIPTORS = 16
const MAX_PROVIDER_DESCRIPTOR_DISPLAY_NAME_BYTES = 256
const MAX_PROVIDER_VENDOR_ID_BYTES = 32
const MAX_CREDENTIAL_LENGTH = 8_192
const CREDENTIAL_STATUSES = ['missing', 'configured', 'unavailable'] as const
const PROVIDER_PROFILE_IDS = [
  'generic_openai_chat',
  'generic_anthropic_messages',
  'deepseek_v4_chat',
  'deepseek_v4_vision',
  'moonshot_k3_chat',
  'moonshot_k2_7_code_chat',
  'moonshot_k2_6_chat'
] as const
const PROVIDER_DIALECTS = ['openai_chat_completions', 'anthropic_messages'] as const
const FORBIDDEN_CREDENTIAL_PROJECTION_KEYS = new Set([
  'apiKey',
  'apiToken',
  'apiTokenOverride',
  'credentialRef',
  'credential_ref',
  'tavilyApiKey'
])

export function parseCredentialStatus(value: unknown, context: string): CredentialStatus {
  return expectEnum(value, CREDENTIAL_STATUSES, context)
}

export function parseCredentialMutation(value: unknown, context: string): CredentialMutation {
  const record = expectRecord(value, context)
  const type = expectEnum(record.type, ['keep', 'replace', 'clear'] as const, `${context}.type`)
  if (type === 'replace') {
    expectOnlyKeys(record, ['type', 'value'] as const, context)
    const credential = expectString(record.value, `${context}.value`)
    const credentialBytes = new TextEncoder().encode(credential).byteLength
    if (credentialBytes === 0 || credentialBytes > MAX_CREDENTIAL_LENGTH) {
      throw invalidProtocolValue(
        `${context}.value`,
        `must contain between 1 and ${MAX_CREDENTIAL_LENGTH} UTF-8 bytes`
      )
    }
    if (/[\p{White_Space}\p{Cc}]/u.test(credential)) {
      throw invalidProtocolValue(`${context}.value`, 'must not contain whitespace or control data')
    }
    return { type, value: credential }
  }
  expectOnlyKeys(record, ['type'] as const, context)
  return { type }
}

function assertSecretFreeProjection(value: unknown, context: string): void {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertSecretFreeProjection(item, `${context}[${index}]`))
    return
  }
  if (typeof value !== 'object' || value === null) return
  for (const [key, nested] of Object.entries(value)) {
    if (FORBIDDEN_CREDENTIAL_PROJECTION_KEYS.has(key)) {
      throw invalidProtocolValue(context, `forbidden credential field ${key}`)
    }
    assertSecretFreeProjection(nested, `${context}.${key}`)
  }
}

function expectNullableString(value: unknown, context: string): string | null {
  return value === null ? null : expectString(value, context)
}

function parseModelSettingsRevision(value: unknown, context: string): string {
  const revision = expectNonEmptyString(value, context)
  if (
    new TextEncoder().encode(revision).byteLength > MAX_MODEL_SETTINGS_REVISION_BYTES ||
    !/^model-settings-v1:[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
      revision
    )
  ) {
    throw invalidProtocolValue(context, 'must be a canonical model settings revision')
  }
  return revision
}

function parseProviderProfileIdentity(value: unknown, context: string) {
  const profile = expectRecord(value, context)
  expectOnlyKeys(profile, ['id', 'version'] as const, context)
  return {
    id: expectNonEmptyString(profile.id, `${context}.id`),
    version: expectSafeInteger(profile.version, `${context}.version`, 1)
  }
}

function parseProviderProfileConfig(value: unknown, context: string): ProviderProfileConfig {
  const record = expectRecord(value, context)
  if (record.schemaVersion === 1) {
    expectOnlyKeys(record, ['schemaVersion', 'profile', 'reasoning'] as const, context)
    const reasoningContext = `${context}.reasoning`
    const reasoning = expectRecord(record.reasoning, reasoningContext)
    expectOnlyKeys(reasoning, ['mode', 'effort'] as const, reasoningContext)
    return {
      schemaVersion: 1,
      profile: parseProviderProfileIdentity(record.profile, `${context}.profile`),
      reasoning: {
        mode: expectEnum(reasoning.mode, REASONING_MODES, `${reasoningContext}.mode`),
        effort: expectEnum(
          reasoning.effort,
          ['provider_default', 'high', 'max'] as const,
          `${reasoningContext}.effort`
        )
      }
    }
  }
  if (record.schemaVersion === 2) {
    expectOnlyKeys(record, ['schemaVersion', 'vendorId', 'profile', 'settings'] as const, context)
    return {
      schemaVersion: 2,
      vendorId: parseProviderVendorId(record.vendorId, `${context}.vendorId`),
      profile: parseProviderProfileIdentity(record.profile, `${context}.profile`),
      settings:
        record.settings &&
        typeof record.settings === 'object' &&
        !Array.isArray(record.settings) &&
        (record.settings as Record<string, unknown>).kind === 'generic'
          ? parseGenericSettings(record.settings, `${context}.settings`)
          : parseProviderFamilySettings(record.settings, `${context}.settings`)
    }
  }
  throw invalidProtocolValue(`${context}.schemaVersion`, 'must be 1 or 2')
}

function parseStorageProviderProfileUpdate(
  value: unknown,
  context: string
): StorageProviderProfileUpdate {
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['unchanged', 'select_generic', 'select_registered_profile', 'select_vendor'] as const,
    `${context}.kind`
  )
  if (kind === 'unchanged' || kind === 'select_generic') {
    expectOnlyKeys(record, ['kind'] as const, context)
    return { kind }
  }
  if (kind === 'select_vendor') {
    expectOnlyKeys(record, ['kind', 'vendorId', 'settings'] as const, context)
    const settings =
      record.settings &&
      typeof record.settings === 'object' &&
      !Array.isArray(record.settings) &&
      (record.settings as Record<string, unknown>).kind === 'generic'
        ? parseGenericSettings(record.settings, `${context}.settings`)
        : parseProviderFamilySettings(record.settings, `${context}.settings`)
    return {
      kind,
      vendorId: parseProviderVendorId(record.vendorId, `${context}.vendorId`),
      settings
    }
  }

  expectOnlyKeys(record, ['kind', 'profileId', 'settings'] as const, context)
  const settingsContext = `${context}.settings`
  const settings = expectRecord(record.settings, settingsContext)
  expectOnlyKeys(settings, ['kind', 'reasoning'] as const, settingsContext)
  if (settings.kind !== 'deepseek_v4_chat') {
    throw invalidProtocolValue(`${settingsContext}.kind`, 'must be deepseek_v4_chat')
  }
  const reasoningContext = `${settingsContext}.reasoning`
  const reasoning = expectRecord(settings.reasoning, reasoningContext)
  expectOnlyKeys(reasoning, ['mode', 'effort'] as const, reasoningContext)
  return {
    kind,
    profileId: expectNonEmptyString(record.profileId, `${context}.profileId`),
    settings: {
      kind: 'deepseek_v4_chat',
      reasoning: {
        mode: expectEnum(reasoning.mode, REASONING_MODES, `${reasoningContext}.mode`),
        effort: expectEnum(
          reasoning.effort,
          ['provider_default', 'high', 'max'] as const,
          `${reasoningContext}.effort`
        )
      }
    }
  }
}

export function parseStorageModelSettingsRecord(value: unknown): StorageModelSettingsRecord {
  const context = 'storage model settings response'
  assertSecretFreeProjection(value, context)
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'configurationRevision',
      'apiUrl',
      'apiTokenStatus',
      'searchMode',
      'tavilyApiKeyStatus',
      'models'
    ] as const,
    context
  )
  const models = expectArray(record.models, `${context}.models`).map((value, index) => {
    const modelContext = `${context}.models[${index}]`
    const model = expectRecord(value, modelContext)
    expectOnlyKeys(
      model,
      [
        'id',
        'providerModelId',
        'displayName',
        'apiUrlOverride',
        'apiTokenOverrideStatus',
        'supportsImage',
        'contextWindowTokens',
        'providerProfileConfig',
        'inputPrice',
        'cachedInputPrice',
        'outputPrice',
        'enabled'
      ] as const,
      modelContext
    )
    return {
      id: expectNonEmptyString(model.id, `${modelContext}.id`),
      providerModelId: expectNonEmptyString(
        model.providerModelId,
        `${modelContext}.providerModelId`
      ),
      displayName: expectNonEmptyString(model.displayName, `${modelContext}.displayName`),
      apiUrlOverride: expectNullableString(model.apiUrlOverride, `${modelContext}.apiUrlOverride`),
      apiTokenOverrideStatus: parseCredentialStatus(
        model.apiTokenOverrideStatus,
        `${modelContext}.apiTokenOverrideStatus`
      ),
      supportsImage: expectBoolean(model.supportsImage, `${modelContext}.supportsImage`),
      contextWindowTokens:
        model.contextWindowTokens === null
          ? null
          : expectSafeInteger(model.contextWindowTokens, `${modelContext}.contextWindowTokens`, 1),
      providerProfileConfig: parseProviderProfileConfig(
        model.providerProfileConfig,
        `${modelContext}.providerProfileConfig`
      ),
      inputPrice: expectString(model.inputPrice, `${modelContext}.inputPrice`),
      cachedInputPrice: expectString(model.cachedInputPrice, `${modelContext}.cachedInputPrice`),
      outputPrice: expectString(model.outputPrice, `${modelContext}.outputPrice`),
      enabled: expectBoolean(model.enabled, `${modelContext}.enabled`)
    }
  })
  return {
    configurationRevision: parseModelSettingsRevision(
      record.configurationRevision,
      `${context}.configurationRevision`
    ),
    apiUrl: expectString(record.apiUrl, `${context}.apiUrl`),
    apiTokenStatus: parseCredentialStatus(record.apiTokenStatus, `${context}.apiTokenStatus`),
    searchMode: expectString(record.searchMode, `${context}.searchMode`),
    tavilyApiKeyStatus: parseCredentialStatus(
      record.tavilyApiKeyStatus,
      `${context}.tavilyApiKeyStatus`
    ),
    models
  }
}

export function parseStorageModelSettingsUpdateRecord(
  value: unknown
): StorageModelSettingsUpdateRecord {
  const context = 'storage model settings update request'
  const record = expectRecord(value, context)
  expectOnlyKeys(
    record,
    [
      'expectedRevision',
      'apiUrl',
      'apiTokenMutation',
      'searchMode',
      'tavilyApiKeyMutation',
      'models'
    ] as const,
    context
  )
  const models = expectArray(record.models, `${context}.models`).map((value, index) => {
    const modelContext = `${context}.models[${index}]`
    const model = expectRecord(value, modelContext)
    expectOnlyKeys(
      model,
      [
        'id',
        'providerModelId',
        'displayName',
        'apiUrlOverride',
        'apiTokenOverrideMutation',
        'supportsImage',
        'contextWindowTokens',
        'providerProfileUpdate',
        'inputPrice',
        'cachedInputPrice',
        'outputPrice',
        'enabled'
      ] as const,
      modelContext
    )
    return {
      id: model.id === null ? null : expectNonEmptyString(model.id, `${modelContext}.id`),
      providerModelId: expectNonEmptyString(
        model.providerModelId,
        `${modelContext}.providerModelId`
      ),
      displayName: expectNonEmptyString(model.displayName, `${modelContext}.displayName`),
      apiUrlOverride: expectNullableString(model.apiUrlOverride, `${modelContext}.apiUrlOverride`),
      apiTokenOverrideMutation: parseCredentialMutation(
        model.apiTokenOverrideMutation,
        `${modelContext}.apiTokenOverrideMutation`
      ),
      supportsImage: expectBoolean(model.supportsImage, `${modelContext}.supportsImage`),
      contextWindowTokens:
        model.contextWindowTokens === null
          ? null
          : expectSafeInteger(model.contextWindowTokens, `${modelContext}.contextWindowTokens`, 1),
      providerProfileUpdate: parseStorageProviderProfileUpdate(
        model.providerProfileUpdate,
        `${modelContext}.providerProfileUpdate`
      ),
      inputPrice: expectString(model.inputPrice, `${modelContext}.inputPrice`),
      cachedInputPrice: expectString(model.cachedInputPrice, `${modelContext}.cachedInputPrice`),
      outputPrice: expectString(model.outputPrice, `${modelContext}.outputPrice`),
      enabled: expectBoolean(model.enabled, `${modelContext}.enabled`)
    }
  })
  return {
    expectedRevision:
      record.expectedRevision === null
        ? null
        : parseModelSettingsRevision(record.expectedRevision, `${context}.expectedRevision`),
    apiUrl: expectString(record.apiUrl, `${context}.apiUrl`),
    apiTokenMutation: parseCredentialMutation(
      record.apiTokenMutation,
      `${context}.apiTokenMutation`
    ),
    searchMode: expectString(record.searchMode, `${context}.searchMode`),
    tavilyApiKeyMutation: parseCredentialMutation(
      record.tavilyApiKeyMutation,
      `${context}.tavilyApiKeyMutation`
    ),
    models
  }
}

const PROVIDER_MODEL_FAMILIES = [
  'generic_openai_chat',
  'generic_anthropic_messages',
  'deepseek_v4_chat',
  'deepseek_v4_vision',
  'moonshot_k3_chat',
  'moonshot_k2_7_code_chat',
  'moonshot_k2_6_chat'
] as const
const REASONING_MODES = ['provider_default', 'enabled', 'disabled'] as const
const REASONING_EFFORTS = ['provider_default', 'low', 'high', 'max'] as const
const MOONSHOT_K26_THINKING_MODES = [
  'provider_default',
  'enabled',
  'disabled',
  'enabled_keep_all'
] as const

function parseProviderVendorId(value: unknown, context: string): ProviderVendorId {
  const vendorId = expectNonEmptyString(value, context)
  if (
    new TextEncoder().encode(vendorId).byteLength > MAX_PROVIDER_VENDOR_ID_BYTES ||
    !/^[a-z0-9._-]+$/.test(vendorId)
  ) {
    throw invalidProtocolValue(context, 'expected a bounded provider vendor identifier')
  }
  return vendorId
}

function expectUniqueEnumArray<const Values extends readonly string[]>(
  value: unknown,
  values: Values,
  context: string
): Values[number][] {
  const result = expectArray(value, context).map((item, index) =>
    expectEnum(item, values, `${context}[${index}]`)
  )
  if (result.length === 0) throw invalidProtocolValue(context, 'must not be empty')
  if (new Set(result).size !== result.length) {
    throw invalidProtocolValue(context, 'must not contain duplicates')
  }
  return result
}

function parseProviderFamilySettings(value: unknown, context: string): ProviderFamilySettings {
  const record = expectRecord(value, context)
  const kind = expectEnum(record.kind, PROVIDER_MODEL_FAMILIES, `${context}.kind`)
  if (kind === 'generic_openai_chat' || kind === 'generic_anthropic_messages') {
    throw invalidProtocolValue(`${context}.kind`, 'generic settings must use kind generic')
  }
  if (kind === 'deepseek_v4_chat' || kind === 'deepseek_v4_vision') {
    expectOnlyKeys(record, ['kind', 'reasoning'] as const, context)
    const reasoningContext = `${context}.reasoning`
    const reasoning = expectRecord(record.reasoning, reasoningContext)
    expectOnlyKeys(reasoning, ['mode', 'effort'] as const, reasoningContext)
    return {
      kind,
      reasoning: {
        mode: expectEnum(reasoning.mode, REASONING_MODES, `${reasoningContext}.mode`),
        effort: expectEnum(reasoning.effort, REASONING_EFFORTS, `${reasoningContext}.effort`)
      }
    }
  }
  if (kind === 'moonshot_k3_chat') {
    expectOnlyKeys(record, ['kind', 'reasoningEffort'] as const, context)
    return {
      kind,
      reasoningEffort: expectEnum(
        record.reasoningEffort,
        REASONING_EFFORTS,
        `${context}.reasoningEffort`
      )
    }
  }
  if (kind === 'moonshot_k2_7_code_chat') {
    expectOnlyKeys(record, ['kind'] as const, context)
    return { kind }
  }
  if (kind === 'moonshot_k2_6_chat') {
    expectOnlyKeys(record, ['kind', 'thinkingMode'] as const, context)
    return {
      kind,
      thinkingMode: expectEnum(
        record.thinkingMode,
        MOONSHOT_K26_THINKING_MODES,
        `${context}.thinkingMode`
      )
    }
  }

  // `generic` is intentionally outside ProviderModelFamilyId and is parsed separately.
  throw invalidProtocolValue(`${context}.kind`, `unexpected value ${String(record.kind)}`)
}

function parseGenericSettings(
  value: unknown,
  context: string
): Extract<ProviderFamilySettings, { kind: 'generic' }> {
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['kind'] as const, context)
  if (record.kind !== 'generic') {
    throw invalidProtocolValue(`${context}.kind`, 'must be generic')
  }
  return { kind: 'generic' }
}

function parseProviderFamilySettingsDescriptor(
  value: unknown,
  context: string
): ProviderFamilySettingsDescriptor {
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['generic', ...PROVIDER_MODEL_FAMILIES.slice(2)] as const,
    `${context}.kind`
  )
  if (kind === 'generic') {
    expectOnlyKeys(record, ['kind', 'defaultSettings'] as const, context)
    return {
      kind,
      defaultSettings: parseGenericSettings(record.defaultSettings, `${context}.defaultSettings`)
    }
  }
  if (kind === 'deepseek_v4_chat' || kind === 'deepseek_v4_vision') {
    expectOnlyKeys(
      record,
      ['kind', 'reasoningModes', 'reasoningEfforts', 'defaultSettings'] as const,
      context
    )
    const reasoningModes = expectUniqueEnumArray(
      record.reasoningModes,
      REASONING_MODES,
      `${context}.reasoningModes`
    ) as ProviderReasoningMode[]
    const reasoningEfforts = expectUniqueEnumArray(
      record.reasoningEfforts,
      REASONING_EFFORTS,
      `${context}.reasoningEfforts`
    ) as ProviderReasoningEffort[]
    const defaultSettings = parseProviderFamilySettings(
      record.defaultSettings,
      `${context}.defaultSettings`
    )
    if (
      defaultSettings.kind !== kind ||
      !reasoningModes.includes(defaultSettings.reasoning.mode) ||
      !reasoningEfforts.includes(defaultSettings.reasoning.effort)
    ) {
      throw invalidProtocolValue(context, 'default DeepSeek settings are outside legal options')
    }
    if (kind === 'deepseek_v4_chat' && defaultSettings.kind === 'deepseek_v4_chat') {
      return { kind, reasoningModes, reasoningEfforts, defaultSettings }
    }
    if (kind === 'deepseek_v4_vision' && defaultSettings.kind === 'deepseek_v4_vision') {
      return { kind, reasoningModes, reasoningEfforts, defaultSettings }
    }
    throw invalidProtocolValue(context, 'default DeepSeek settings use the wrong family')
  }
  if (kind === 'moonshot_k3_chat') {
    expectOnlyKeys(record, ['kind', 'reasoningEfforts', 'defaultSettings'] as const, context)
    const reasoningEfforts = expectUniqueEnumArray(
      record.reasoningEfforts,
      REASONING_EFFORTS,
      `${context}.reasoningEfforts`
    ) as ProviderReasoningEffort[]
    const defaultSettings = parseProviderFamilySettings(
      record.defaultSettings,
      `${context}.defaultSettings`
    )
    if (
      defaultSettings.kind !== kind ||
      !reasoningEfforts.includes(defaultSettings.reasoningEffort)
    ) {
      throw invalidProtocolValue(context, 'default Moonshot settings are outside legal options')
    }
    return { kind, reasoningEfforts, defaultSettings }
  }
  if (kind === 'moonshot_k2_7_code_chat') {
    expectOnlyKeys(record, ['kind', 'defaultSettings'] as const, context)
    const defaultSettings = parseProviderFamilySettings(
      record.defaultSettings,
      `${context}.defaultSettings`
    )
    if (defaultSettings.kind !== kind) {
      throw invalidProtocolValue(context, 'default Moonshot settings use the wrong family')
    }
    return { kind, defaultSettings }
  }
  expectOnlyKeys(record, ['kind', 'thinkingModes', 'defaultSettings'] as const, context)
  const thinkingModes = expectUniqueEnumArray(
    record.thinkingModes,
    MOONSHOT_K26_THINKING_MODES,
    `${context}.thinkingModes`
  )
  const defaultSettings = parseProviderFamilySettings(
    record.defaultSettings,
    `${context}.defaultSettings`
  )
  if (
    defaultSettings.kind !== 'moonshot_k2_6_chat' ||
    !thinkingModes.includes(defaultSettings.thinkingMode)
  ) {
    throw invalidProtocolValue(context, 'default Moonshot settings are outside legal options')
  }
  return { kind: 'moonshot_k2_6_chat', thinkingModes, defaultSettings }
}

export function parseProviderVendorDescriptors(value: unknown): ProviderVendorDescriptor[] {
  const context = 'Provider vendor descriptors'
  const records = expectArray(value, context)
  if (records.length > MAX_PROVIDER_DESCRIPTORS) {
    throw invalidProtocolValue(context, `must not exceed ${MAX_PROVIDER_DESCRIPTORS} entries`)
  }
  const result = records.map((value, index) => {
    const itemContext = `${context}[${index}]`
    const record = expectRecord(value, itemContext)
    expectOnlyKeys(record, ['vendorId', 'displayName', 'selectable'] as const, itemContext)
    return {
      vendorId: parseProviderVendorId(record.vendorId, `${itemContext}.vendorId`),
      displayName: expectNonEmptyString(record.displayName, `${itemContext}.displayName`),
      selectable: expectBoolean(record.selectable, `${itemContext}.selectable`)
    }
  })
  if (new Set(result.map(({ vendorId }) => vendorId)).size !== result.length) {
    throw invalidProtocolValue(context, 'must not contain duplicate vendor ids')
  }
  return result
}

export function parseProviderProfileUiDescriptors(value: unknown): ProviderProfileUiDescriptor[] {
  const context = 'Provider Profile UI descriptors'
  assertSecretFreeProjection(value, context)
  const records = expectArray(value, context)
  if (records.length > MAX_PROVIDER_DESCRIPTORS) {
    throw invalidProtocolValue(context, `must not exceed ${MAX_PROVIDER_DESCRIPTORS} entries`)
  }
  const result = records.map((value, index) => {
    const itemContext = `${context}[${index}]`
    const record = expectRecord(value, itemContext)
    expectOnlyKeys(
      record,
      [
        'profileId',
        'profileVersion',
        'displayName',
        'compatibleDialects',
        'settingsKind',
        'selectable'
      ] as const,
      itemContext
    )
    const displayName = expectNonEmptyString(record.displayName, `${itemContext}.displayName`)
    if (
      new TextEncoder().encode(displayName).byteLength > MAX_PROVIDER_DESCRIPTOR_DISPLAY_NAME_BYTES
    ) {
      throw invalidProtocolValue(
        `${itemContext}.displayName`,
        `must not exceed ${MAX_PROVIDER_DESCRIPTOR_DISPLAY_NAME_BYTES} UTF-8 bytes`
      )
    }
    return {
      profileId: expectEnum(record.profileId, PROVIDER_PROFILE_IDS, `${itemContext}.profileId`),
      profileVersion: expectSafeInteger(record.profileVersion, `${itemContext}.profileVersion`, 1),
      displayName,
      compatibleDialects: expectUniqueEnumArray(
        record.compatibleDialects,
        PROVIDER_DIALECTS,
        `${itemContext}.compatibleDialects`
      ),
      settingsKind: expectEnum(
        record.settingsKind,
        ['none', 'deepseek_v4_chat'] as const,
        `${itemContext}.settingsKind`
      ),
      selectable: expectBoolean(record.selectable, `${itemContext}.selectable`)
    }
  })
  if (new Set(result.map(({ profileId }) => profileId)).size !== result.length) {
    throw invalidProtocolValue(context, 'must not contain duplicate Profile ids')
  }
  return result
}

export function parseProviderVendorModelPolicyDescriptor(
  value: unknown
): ProviderVendorModelPolicyDescriptor {
  const context = 'Provider vendor model policy descriptor'
  const record = expectRecord(value, context)
  const status = expectEnum(
    record.status,
    ['supported', 'unsupported'] as const,
    `${context}.status`
  )
  const vendorId = parseProviderVendorId(record.vendorId, `${context}.vendorId`)
  if (status === 'unsupported') {
    expectOnlyKeys(record, ['status', 'vendorId', 'reason'] as const, context)
    return {
      status,
      vendorId,
      reason: expectEnum(
        record.reason,
        ['unsupported_vendor', 'unsupported_model', 'unsupported_dialect'] as const,
        `${context}.reason`
      )
    }
  }

  expectOnlyKeys(
    record,
    ['status', 'vendorId', 'modelFamily', 'settingsKind', 'imageInput', 'settings'] as const,
    context
  )
  const modelFamily = expectEnum(
    record.modelFamily,
    PROVIDER_MODEL_FAMILIES,
    `${context}.modelFamily`
  ) as ProviderModelFamilyId
  const settingsKind = expectEnum(
    record.settingsKind,
    ['none', 'deepseek', 'moonshot'] as const,
    `${context}.settingsKind`
  )
  const imageInput = expectEnum(
    record.imageInput,
    ['user_configurable', 'supported', 'unsupported'] as const,
    `${context}.imageInput`
  )
  const settings = parseProviderFamilySettingsDescriptor(record.settings, `${context}.settings`)
  const isGeneric =
    modelFamily === 'generic_openai_chat' || modelFamily === 'generic_anthropic_messages'
  const isDeepSeek = modelFamily === 'deepseek_v4_chat' || modelFamily === 'deepseek_v4_vision'
  const valid = isGeneric
    ? vendorId === 'generic' && settingsKind === 'none' && settings.kind === 'generic'
    : isDeepSeek
      ? vendorId === 'deepseek' && settingsKind === 'deepseek' && settings.kind === modelFamily
      : vendorId === 'moonshot' && settingsKind === 'moonshot' && settings.kind === modelFamily
  if (!valid)
    throw invalidProtocolValue(context, 'vendor, family, settings, and image policy disagree')

  return { status, vendorId, modelFamily, settingsKind, imageInput, settings }
}

function expectBoundedForkIdentifier(value: unknown, context: string): string {
  const identifier = expectNonEmptyString(value, context)
  if (new TextEncoder().encode(identifier).byteLength > MAX_FORK_IDENTIFIER_BYTES) {
    throw invalidProtocolValue(context, `must not exceed ${MAX_FORK_IDENTIFIER_BYTES} UTF-8 bytes`)
  }
  return identifier
}

function parseStorageConversationForkPoint(value: unknown): StorageConversationForkPoint {
  const context = 'storage fork conversation request.forkPoint'
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    [
      'assistant_reply',
      'provider_transition_boundary',
      'manual_compaction_boundary',
      'latest'
    ] as const,
    `${context}.kind`
  )

  if (kind === 'latest') {
    expectOnlyKeys(record, ['kind'], context)
    return { kind }
  }

  if (kind === 'assistant_reply') {
    expectOnlyKeys(record, ['kind', 'assistantMessageId'] as const, context)
    return {
      kind,
      assistantMessageId: expectBoundedForkIdentifier(
        record.assistantMessageId,
        `${context}.assistantMessageId`
      )
    }
  }

  expectOnlyKeys(record, ['kind', 'operationId'] as const, context)
  const operationId = expectBoundedForkIdentifier(record.operationId, `${context}.operationId`)
  if (
    kind === 'manual_compaction_boundary' &&
    (operationId.trim() !== operationId || [...operationId].length > 1024)
  ) {
    throw invalidProtocolValue(
      `${context}.operationId`,
      'must be an unpadded identifier of at most 1024 characters'
    )
  }
  return {
    kind,
    operationId
  }
}

export function parseStorageForkConversationRequest(
  value: unknown
): StorageForkConversationRequest {
  const context = 'storage fork conversation request'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['requestId', 'sourceConversationId', 'forkPoint'] as const, context)

  const requestId = expectBoundedForkIdentifier(record.requestId, `${context}.requestId`)
  const sourceConversationId = expectBoundedForkIdentifier(
    record.sourceConversationId,
    `${context}.sourceConversationId`
  )
  return {
    requestId,
    sourceConversationId,
    forkPoint: parseStorageConversationForkPoint(record.forkPoint)
  }
}

export function parseStorageForkConversationErrorData(
  value: unknown
): StorageForkConversationErrorData {
  const context = 'storage fork conversation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['type', 'code', 'conversationId', 'activeSessionCount'] as const, context)

  const conversationId = expectNonEmptyString(record.conversationId, `${context}.conversationId`)
  if (new TextEncoder().encode(conversationId).byteLength > MAX_CONVERSATION_ID_BYTES) {
    throw invalidProtocolValue(
      `${context}.conversationId`,
      `must not exceed ${MAX_CONVERSATION_ID_BYTES} UTF-8 bytes`
    )
  }

  const activeSessionCount = expectSafeInteger(
    record.activeSessionCount,
    `${context}.activeSessionCount`,
    1
  )
  if (activeSessionCount > MAX_ACTIVE_COMMAND_SESSIONS) {
    throw invalidProtocolValue(
      `${context}.activeSessionCount`,
      `must not exceed ${MAX_ACTIVE_COMMAND_SESSIONS}`
    )
  }

  return {
    type: expectEnum(record.type, ['conversation_fork'] as const, `${context}.type`),
    code: expectEnum(record.code, ['active_command_session'] as const, `${context}.code`),
    conversationId,
    activeSessionCount
  }
}

export function parseStorageModelSettingsValidationErrorData(
  value: unknown
): StorageModelSettingsValidationErrorData {
  const context = 'storage model settings validation error data'
  const record = expectRecord(value, context)
  expectOnlyKeys(record, ['kind', 'code', 'displayName'] as const, context)

  const displayName = expectNonEmptyString(record.displayName, `${context}.displayName`)
  if (new TextEncoder().encode(displayName).byteLength > MAX_MODEL_DISPLAY_NAME_BYTES) {
    throw invalidProtocolValue(
      `${context}.displayName`,
      `must not exceed ${MAX_MODEL_DISPLAY_NAME_BYTES} UTF-8 bytes`
    )
  }

  return {
    kind: expectEnum(record.kind, ['model_settings_validation'] as const, `${context}.kind`),
    code: expectEnum(record.code, ['duplicate_display_name'] as const, `${context}.code`),
    displayName
  }
}
