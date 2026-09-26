import {
  expectRecord,
  expectOnlyKeys,
  expectNonEmptyString,
  expectSafeInteger,
  expectEnum,
  invalidProtocolValue,
  expectArray,
  expectBoolean
} from '../skills/validation'
import type {
  ProviderProfileConfig,
  StorageProviderProfileUpdate,
  ProviderVendorId,
  ProviderFamilyReasoningPolicy,
  ProviderFamilySettings,
  ProviderFamilySettingsDescriptor,
  ProviderReasoningMode,
  ProviderReasoningEffort,
  ProviderVendorDescriptor,
  ProviderProfileUiDescriptor,
  ProviderVendorModelPolicyDescriptor,
  ProviderModelFamilyId
} from '../storage'
import { assertSecretFreeProjection } from './validation'

const MAX_PROVIDER_DESCRIPTORS = 16

const MAX_PROVIDER_DESCRIPTOR_DISPLAY_NAME_BYTES = 256

const MAX_PROVIDER_VENDOR_ID_BYTES = 32

const PROVIDER_PROFILE_IDS = [
  'generic_openai_chat',
  'generic_anthropic_messages',
  'deepseek_v4_1_flash_chat',
  'deepseek_v4_pro_0813_chat',
  'moonshot_k3_chat',
  'moonshot_k2_7_code_chat',
  'moonshot_k2_6_chat'
] as const

const PROVIDER_DIALECTS = ['openai_chat_completions', 'anthropic_messages'] as const

function parseProviderProfileIdentity(value: unknown, context: string) {
  const profile = expectRecord(value, context)
  expectOnlyKeys(profile, ['id', 'version'] as const, context)
  return {
    id: expectNonEmptyString(profile.id, `${context}.id`),
    version: expectSafeInteger(profile.version, `${context}.version`, 1)
  }
}

export function parseProviderProfileConfig(value: unknown, context: string): ProviderProfileConfig {
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

export function parseStorageProviderProfileUpdate(
  value: unknown,
  context: string
): StorageProviderProfileUpdate {
  const record = expectRecord(value, context)
  const kind = expectEnum(
    record.kind,
    ['unchanged', 'select_generic', 'select_vendor'] as const,
    `${context}.kind`
  )
  if (kind === 'unchanged' || kind === 'select_generic') {
    expectOnlyKeys(record, ['kind'] as const, context)
    return { kind }
  }
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

const PROVIDER_MODEL_FAMILIES = [
  'generic_openai_chat',
  'generic_anthropic_messages',
  'deepseek_flash_chat',
  'deepseek_pro_chat',
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

function parseProviderFamilyReasoningPolicy(
  value: unknown,
  context: string
): ProviderFamilyReasoningPolicy {
  const reasoning = expectRecord(value, context)
  expectOnlyKeys(reasoning, ['mode', 'effort'] as const, context)
  const mode = expectEnum(reasoning.mode, REASONING_MODES, `${context}.mode`)
  const effort = expectEnum(reasoning.effort, REASONING_EFFORTS, `${context}.effort`)
  if (mode === 'disabled') {
    if (effort !== 'provider_default') {
      throw invalidProtocolValue(context, 'disabled reasoning must use provider_default effort')
    }
    return { mode, effort }
  }
  return { mode, effort }
}

function parseProviderFamilySettings(value: unknown, context: string): ProviderFamilySettings {
  const record = expectRecord(value, context)
  const kind = expectEnum(record.kind, PROVIDER_MODEL_FAMILIES, `${context}.kind`)
  if (kind === 'generic_openai_chat' || kind === 'generic_anthropic_messages') {
    throw invalidProtocolValue(`${context}.kind`, 'generic settings must use kind generic')
  }
  if (kind === 'deepseek_flash_chat' || kind === 'deepseek_pro_chat') {
    expectOnlyKeys(record, ['kind', 'reasoning'] as const, context)
    return {
      kind,
      reasoning: parseProviderFamilyReasoningPolicy(record.reasoning, `${context}.reasoning`)
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
  if (kind === 'deepseek_flash_chat' || kind === 'deepseek_pro_chat') {
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
    if (kind === 'deepseek_flash_chat' && defaultSettings.kind === 'deepseek_flash_chat') {
      return { kind, reasoningModes, reasoningEfforts, defaultSettings }
    }
    if (kind === 'deepseek_pro_chat' && defaultSettings.kind === 'deepseek_pro_chat') {
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
        ['none'] as const,
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
  const isDeepSeek = modelFamily === 'deepseek_flash_chat' || modelFamily === 'deepseek_pro_chat'
  const valid = isGeneric
    ? vendorId === 'generic' && settingsKind === 'none' && settings.kind === 'generic'
    : isDeepSeek
      ? vendorId === 'deepseek' && settingsKind === 'deepseek' && settings.kind === modelFamily
      : vendorId === 'moonshot' && settingsKind === 'moonshot' && settings.kind === modelFamily
  if (!valid)
    throw invalidProtocolValue(context, 'vendor, family, settings, and image policy disagree')

  return { status, vendorId, modelFamily, settingsKind, imageInput, settings }
}
