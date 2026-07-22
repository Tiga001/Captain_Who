export const IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION = 1 as const
export const IMAGE_GENERATION_CONFIGURATION_ERROR_CODE = -32020 as const

export const IMAGE_GENERATION_GET_CONFIGURATION_METHOD = 'imageGeneration.getConfiguration' as const
export const IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD =
  'imageGeneration.updateConfiguration' as const
export const IMAGE_GENERATION_SET_ENABLED_METHOD = 'imageGeneration.setEnabled' as const
export const IMAGE_GENERATION_GET_STATUS_METHOD = 'imageGeneration.getStatus' as const

/**
 * Stable identifier for the backend adapter that owns provider-specific request mapping.
 *
 * The first protocol version deliberately exposes only the adapter implemented by the backend.
 * Adding another adapter is a protocol change, not an invitation for clients to send arbitrary
 * provider identifiers.
 */
export type ImageGenerationAdapterId = 'smartmlSeedream'

/** User-declared capabilities of the configured endpoint and model pair. */
export interface ImageGenerationCapabilities {
  /** Text-to-image is the mandatory baseline capability and cannot be disabled. */
  textToImage: true
  /** Enabling this flag authorizes the backend to expose image-input operations. */
  imageToImage: boolean
}

/**
 * Provider-independent defaults supported by the first backend adapter.
 *
 * Keeping these fields typed prevents raw provider parameters from crossing the Host boundary.
 */
export interface ImageGenerationDefaults {
  sizePreset: '2K'
  watermark: boolean
}

/** Only secret-free availability state is observable; credential material is never returned. */
export type ImageGenerationCredentialStatus = 'missing' | 'configured' | 'unavailable'

/**
 * Backend-authoritative effective readiness. `readyUnverified` means configuration-complete, not
 * that a paid provider request or health probe has succeeded.
 */
export type ImageGenerationReadiness =
  | 'disabled'
  | 'missingEndpoint'
  | 'missingModel'
  | 'missingCredential'
  | 'credentialUnavailable'
  | 'readyUnverified'

/** Public, secret-free snapshot of the single configured image-generation profile. */
export interface ImageGenerationConfiguration {
  adapterId: ImageGenerationAdapterId
  endpointUrl: string
  modelId: string
  capabilities: ImageGenerationCapabilities
  defaults: ImageGenerationDefaults
  credentialStatus: ImageGenerationCredentialStatus
  enabled: boolean
  readiness: ImageGenerationReadiness
  /** Opaque compare-and-swap token. Clients must not derive meaning from it. */
  revision: string
}

export interface ImageGenerationGetConfigurationOutput {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  configuration: ImageGenerationConfiguration
}

/**
 * Secrets use an explicit mutation union so omitted or stale form state can never erase a key.
 */
export type ImageGenerationCredentialMutation =
  { type: 'keep' } | { type: 'replace'; value: string } | { type: 'clear' }

/**
 * Replaces all non-enablement fields in one compare-and-swap operation. Empty endpoint/model
 * values are valid only as disabled, incomplete configuration and are validated by the service.
 */
export interface ImageGenerationUpdateConfigurationInput {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  expectedRevision: string
  adapterId: ImageGenerationAdapterId
  endpointUrl: string
  modelId: string
  capabilities: ImageGenerationCapabilities
  defaults: ImageGenerationDefaults
  credentialMutation: ImageGenerationCredentialMutation
}

export type ImageGenerationConfigurationMutationOutcome = 'updated' | 'alreadyCurrent'

export interface ImageGenerationUpdateConfigurationOutput {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  outcome: ImageGenerationConfigurationMutationOutcome
  configuration: ImageGenerationConfiguration
}

export interface ImageGenerationSetEnabledInput {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  expectedRevision: string
  enabled: boolean
}

export interface ImageGenerationSetEnabledOutput {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  outcome: ImageGenerationConfigurationMutationOutcome
  configuration: ImageGenerationConfiguration
}

/**
 * Compact operational projection. It intentionally omits endpoint, model, and credential values;
 * callers needing editable configuration must use getConfiguration.
 */
export interface ImageGenerationStatus {
  schemaVersion: typeof IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION
  adapterId: ImageGenerationAdapterId
  configurationRevision: string
  enabled: boolean
  readiness: ImageGenerationReadiness
  credentialStatus: ImageGenerationCredentialStatus
  capabilities: ImageGenerationCapabilities
  defaults: ImageGenerationDefaults
}

export type ImageGenerationConfigurationOperation =
  'getConfiguration' | 'updateConfiguration' | 'setEnabled' | 'getStatus'

export type ImageGenerationConfigurationErrorCode =
  | 'invalidRequest'
  | 'revisionConflict'
  | 'unsupportedAdapter'
  | 'missingEndpoint'
  | 'invalidEndpoint'
  | 'insecureEndpoint'
  | 'missingModel'
  | 'invalidModelId'
  | 'missingCredential'
  | 'invalidCredential'
  | 'credentialReplacementRequired'
  | 'storageUnavailable'
  | 'credentialStoreUnavailable'
  | 'commitIndeterminate'
  | 'unavailable'

export type ImageGenerationConfigurationRecovery =
  'fixConfiguration' | 'refreshConfiguration' | 'reenterCredential' | 'retry' | 'contactSupport'

/** Structured JSON-RPC error data for every configuration Host operation. */
export interface ImageGenerationConfigurationErrorData {
  type: 'imageGenerationConfiguration'
  operation: ImageGenerationConfigurationOperation
  code: ImageGenerationConfigurationErrorCode
  recovery: ImageGenerationConfigurationRecovery
  message: string
  /** Returned on a CAS conflict so clients know their snapshot is stale. */
  currentRevision?: string
  retryAfterMs?: number
  /** A refresh is mandatory before retrying an indeterminate persistence result. */
  configurationMayHaveChanged?: true
}
