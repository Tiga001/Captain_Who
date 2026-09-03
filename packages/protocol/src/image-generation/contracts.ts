import type { CredentialMutation, CredentialStatus } from '../storage'

export const IMAGE_GENERATION_CONFIGURATION_SCHEMA_VERSION = 2 as const
export const IMAGE_GENERATION_CONFIGURATION_ERROR_CODE = -32020 as const

export const IMAGE_GENERATION_GET_CONFIGURATION_METHOD = 'imageGeneration.getConfiguration' as const
export const IMAGE_GENERATION_UPDATE_CONFIGURATION_METHOD =
  'imageGeneration.updateConfiguration' as const
export const IMAGE_GENERATION_SET_ENABLED_METHOD = 'imageGeneration.setEnabled' as const
export const IMAGE_GENERATION_GET_STATUS_METHOD = 'imageGeneration.getStatus' as const
export const IMAGE_GENERATION_READ_ARTIFACT_METHOD = 'imageGeneration.readArtifact' as const
export const IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION = 1 as const
export const IMAGE_GENERATION_ARTIFACT_ERROR_CODE = -32021 as const

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

/** Secret-free availability state used outside the explicit settings-edit response. */
export type ImageGenerationCredentialStatus = CredentialStatus

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
export type ImageGenerationCredentialMutation = CredentialMutation

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

/**
 * Version of the presentation-safe image-generation result emitted by the Agent Tool.
 *
 * This boundary deliberately contains no provider endpoint, credential, provider output URL,
 * filesystem path, or input image bytes. Generated content is referenced exclusively through an
 * application-owned immutable Artifact URI.
 */
export const AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION = 1 as const

export type AgentImageGenerationResultStatus =
  'succeeded' | 'failed' | 'cancelled' | 'outcomeIndeterminate' | 'commitIndeterminate'

export type AgentImageGenerationOperation = 'generate' | 'edit'

export type AgentImageGenerationArtifactKind = 'image'

export type AgentImageGenerationArtifactFormat = 'png' | 'jpeg' | 'webp'

/** Immutable identity and display metadata for a verified managed image Artifact. */
export interface AgentImageGenerationArtifact {
  artifactId: string
  uri: string
  kind: AgentImageGenerationArtifactKind
  format: AgentImageGenerationArtifactFormat
  mimeType: string
  width: number
  height: number
  sizeBytes: number
  sha256: string
}

/** Immutable identity of a conversation-authorized PDF published by a managed command. */
export interface AgentManagedDocumentArtifact {
  artifactId: string
  uri: string
  kind: 'document'
  format: 'pdf'
  mimeType: 'application/pdf'
  sizeBytes: number
  sha256: string
}

/** Existing read transport generalized without adding another Host method. */
export type ManagedArtifactReadIdentity =
  AgentImageGenerationArtifact | AgentManagedDocumentArtifact

/**
 * Reads one immutable generated image or managed-command PDF persisted in an Agent result.
 *
 * Supplying the complete frozen identity lets the backend reject stale or tampered history before
 * returning content. Provider URLs and managed filesystem paths are never accepted as locators.
 */
export interface ImageGenerationArtifactReadInput {
  schemaVersion: typeof IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION
  artifact: ManagedArtifactReadIdentity
  /**
   * Optional authority for generic managed-command Artifacts. Legacy image-generation Artifacts
   * remain readable without it; command-produced images and documents require an exact grant.
   */
  conversationId?: string
  /**
   * Exact root authority for a read-only child Conversation observer. This field never grants
   * access by itself: the backend verifies that `conversationId` is a direct or transitive child
   * in this root tree before consulting the immutable Artifact grant.
   */
  observerRootConversationId?: string
}

/**
 * Core JSON-RPC transport representation. Main validates and decodes `dataBase64` before exposing
 * a typed byte array through the Renderer Host API.
 */
export interface ImageGenerationArtifactReadOutput {
  schemaVersion: typeof IMAGE_GENERATION_ARTIFACT_CONTENT_SCHEMA_VERSION
  artifact: ManagedArtifactReadIdentity
  fileName: string
  dataBase64: string
}

export type ImageGenerationArtifactOperation = 'read'

export type ImageGenerationArtifactErrorCode =
  'invalidRequest' | 'notFound' | 'integrityCheckFailed' | 'tooLarge' | 'unavailable'

export type ImageGenerationArtifactRecovery = 'doNotRetry' | 'retry' | 'regenerate'

/** Bounded, path-free structured failure for generated-image content reads. */
export interface ImageGenerationArtifactErrorData {
  type: 'imageGenerationArtifact'
  operation: ImageGenerationArtifactOperation
  code: ImageGenerationArtifactErrorCode
  recovery: ImageGenerationArtifactRecovery
  message: string
  retryable: boolean
}

/** Correlation metadata safe to persist in Agent traces and expose to clients. */
export interface AgentImageGenerationAudit {
  executionId: string
  requestFingerprint: string
  providerProfileId: string
  adapterId: string
  profileRevision: number
  modelId: string
  providerRequestId?: string
  httpStatus?: number
  createdAt: number
  completedAt: number
  durationMs: number
}

export type AgentImageGenerationFailureCode =
  | 'providerFailed'
  | 'cancelled'
  | 'deadlineExceeded'
  | 'artifactFailed'
  | 'commitIndeterminate'
  | 'executionInterrupted'
  | 'journalUnavailable'

export type AgentImageGenerationFailurePhase =
  | 'admission'
  | 'configuration'
  | 'provider'
  | 'artifactDownload'
  | 'artifactPublish'
  | 'journal'
  | 'recovery'

/** Stable failure details with explicit remote-generation and Artifact-commit uncertainty. */
export interface AgentImageGenerationFailure {
  code: AgentImageGenerationFailureCode
  phase: AgentImageGenerationFailurePhase
  message: string
  recovery: string
  retryable: boolean
  generationMayHaveSucceeded: boolean
  providerSucceeded: boolean
  artifactCommitMayHaveSucceeded: boolean
}

interface AgentImageGenerationResultBase {
  schemaVersion: typeof AGENT_IMAGE_GENERATION_RESULT_SCHEMA_VERSION
  operation: AgentImageGenerationOperation
  audit: AgentImageGenerationAudit
}

export interface AgentImageGenerationSucceededResult extends AgentImageGenerationResultBase {
  status: 'succeeded'
  artifact: AgentImageGenerationArtifact
  failure?: never
}

export interface AgentImageGenerationUnsuccessfulResult extends AgentImageGenerationResultBase {
  status: Exclude<AgentImageGenerationResultStatus, 'succeeded'>
  artifact?: never
  failure: AgentImageGenerationFailure
}

/**
 * Terminal image-generation Tool result. The discriminated union makes Artifact publication
 * impossible to confuse with a failed, cancelled, or indeterminate execution.
 */
export type AgentImageGenerationResult =
  AgentImageGenerationSucceededResult | AgentImageGenerationUnsuccessfulResult
