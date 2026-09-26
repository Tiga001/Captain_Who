import type {
  CredentialStatus,
  CredentialMutation,
  StorageModelExecutionStatus,
  StorageModelSettingsRecord,
  StorageModelSettingsUpdateRecord,
  StorageModelSettingsValidationErrorData
} from '../storage'
import {
  expectEnum,
  expectRecord,
  expectOnlyKeys,
  expectString,
  invalidProtocolValue,
  expectNonEmptyString,
  expectArray,
  expectBoolean,
  expectSafeInteger
} from '../skills/validation'
import { assertSecretFreeProjection, expectNullableString } from './validation'
import { parseProviderProfileConfig, parseStorageProviderProfileUpdate } from './provider'

const MAX_MODEL_DISPLAY_NAME_BYTES = 512

const MAX_MODEL_CONFIG_ID_BYTES = 2_048

const MAX_MODEL_SETTINGS_REVISION_BYTES = 128

const MAX_CREDENTIAL_LENGTH = 8_192

const CREDENTIAL_STATUSES = ['missing', 'configured', 'unavailable'] as const

const MODEL_EXECUTION_STATUSES = ['available', 'unavailable'] as const

const MODEL_UNAVAILABLE_REASONS = [
  'settings_missing',
  'not_found',
  'disabled',
  'invalid_connection',
  'invalid_profile',
  'missing_connection_identity',
  'missing_protocol_identity',
  'unsupported_runtime',
  'capabilities_changed',
  'credential_missing',
  'credential_unavailable'
] as const

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

function parseStorageModelExecution(value: unknown, context: string): StorageModelExecutionStatus {
  const record = expectRecord(value, context)
  const status = expectEnum(record.status, MODEL_EXECUTION_STATUSES, `${context}.status`)
  if (status === 'available') {
    expectOnlyKeys(record, ['status'] as const, context)
    return { status }
  }
  expectOnlyKeys(record, ['status', 'reason'] as const, context)
  return {
    status,
    reason: expectEnum(record.reason, MODEL_UNAVAILABLE_REASONS, `${context}.reason`)
  }
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
        'enabled',
        'execution'
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
      enabled: expectBoolean(model.enabled, `${modelContext}.enabled`),
      execution: parseStorageModelExecution(model.execution, `${modelContext}.execution`)
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
      'validateContextCapacityModelId',
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
  let validateContextCapacityModelId: string | undefined
  if (record.validateContextCapacityModelId !== undefined) {
    validateContextCapacityModelId = expectNonEmptyString(
      record.validateContextCapacityModelId,
      `${context}.validateContextCapacityModelId`
    )
    if (
      new TextEncoder().encode(validateContextCapacityModelId).byteLength >
        MAX_MODEL_CONFIG_ID_BYTES ||
      !models.some((model) => model.id === validateContextCapacityModelId)
    ) {
      throw invalidProtocolValue(
        `${context}.validateContextCapacityModelId`,
        'must identify an existing model in this save request'
      )
    }
  }
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
    models,
    ...(validateContextCapacityModelId === undefined ? {} : { validateContextCapacityModelId })
  }
}

export function parseStorageModelSettingsValidationErrorData(
  value: unknown
): StorageModelSettingsValidationErrorData {
  const context = 'storage model settings validation error data'
  const record = expectRecord(value, context)
  const kind = expectEnum(record.kind, ['model_settings_validation'] as const, `${context}.kind`)
  const code = expectEnum(
    record.code,
    ['duplicate_display_name', 'invalid_context_capacity_configuration'] as const,
    `${context}.code`
  )
  expectOnlyKeys(
    record,
    code === 'duplicate_display_name'
      ? ['kind', 'code', 'displayName']
      : [
          'kind',
          'code',
          'displayName',
          'modelId',
          'contextWindowTokens',
          'reservedOutputTokens',
          'safetyMarginTokens',
          'minimumContextWindowTokens'
        ],
    context
  )

  const displayName = expectNonEmptyString(record.displayName, `${context}.displayName`)
  if (new TextEncoder().encode(displayName).byteLength > MAX_MODEL_DISPLAY_NAME_BYTES) {
    throw invalidProtocolValue(
      `${context}.displayName`,
      `must not exceed ${MAX_MODEL_DISPLAY_NAME_BYTES} UTF-8 bytes`
    )
  }

  if (code === 'duplicate_display_name') return { kind, code, displayName }

  const modelId = expectNonEmptyString(record.modelId, `${context}.modelId`)
  if (new TextEncoder().encode(modelId).byteLength > MAX_MODEL_CONFIG_ID_BYTES) {
    throw invalidProtocolValue(`${context}.modelId`, 'model id exceeds maximum size')
  }
  const tokenCount = (key: string) => {
    const count = expectSafeInteger(record[key], `${context}.${key}`, 1)
    if (count > 4_294_967_295) {
      throw invalidProtocolValue(`${context}.${key}`, 'must not exceed u32 token range')
    }
    return count
  }
  return {
    kind,
    code,
    modelId,
    displayName,
    contextWindowTokens: tokenCount('contextWindowTokens'),
    reservedOutputTokens: tokenCount('reservedOutputTokens'),
    safetyMarginTokens: tokenCount('safetyMarginTokens'),
    minimumContextWindowTokens: tokenCount('minimumContextWindowTokens')
  }
}
