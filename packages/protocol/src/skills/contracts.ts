export const SKILL_CATALOG_SCHEMA_VERSION = 4 as const
export const SKILL_MUTATION_SCHEMA_VERSION = 1 as const
export const SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION = 1 as const
export const SKILL_MANAGEMENT_SCHEMA_VERSION = 1 as const
export const SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION = 2 as const
export const SKILL_INSTALLATION_ERROR_CODE = -32010 as const
export const SKILL_INSPECTION_ERROR_CODE = -32011 as const
export const SKILL_MANAGEMENT_ERROR_CODE = -32012 as const
export const SKILL_SOURCE_RESOLUTION_ERROR_CODE = -32013 as const

export const SKILLS_INSTALL_LOCAL_METHOD = 'skills.installLocal' as const
export const SKILLS_UPDATE_LOCAL_METHOD = 'skills.updateLocal' as const
export const SKILLS_UNINSTALL_METHOD = 'skills.uninstall' as const
export const SKILLS_INSPECT_INSTALLATION_METHOD = 'skills.inspectInstallation' as const
export const SKILLS_COMMIT_INSTALLATION_METHOD = 'skills.commitInstallation' as const
export const SKILLS_CANCEL_PREPARATION_METHOD = 'skills.cancelPreparation' as const
export const SKILLS_LIST_MANAGEMENT_METHOD = 'skills.listManagement' as const
export const SKILLS_SET_ENABLED_METHOD = 'skills.setEnabled' as const
export const SKILLS_CHANGED_NOTIFICATION_METHOD = 'skills.changed' as const
export const SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD = 'skills.resolveInstallationSource' as const
export const SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD = 'skills.cancelSourceResolution' as const

export type SkillSourceKind = 'workspace' | 'bundled' | 'installed'

export type SkillSourceDescriptor =
  | {
      kind: 'workspace'
      /** Opaque identity of the workspace source. */
      id: string
    }
  | {
      kind: 'bundled'
      /** Opaque identity of the application-owned bundle. */
      id: string
    }
  | {
      kind: 'installed'
      /** Opaque identity of an application-managed installation source. */
      id: string
    }

/** Skill instructions never grant permissions, regardless of where they came from. */
export type SkillTrust = 'untrusted' | 'application'

/** V1 activations are intentionally scoped to one complete agent run. */
export type SkillActivationScope = 'run'

export type SkillRecovery = 'retrySameSelection' | 'refreshCatalog' | 'rejectSelection'

export interface SkillSelection {
  id: string
  revision: string
}

export type SkillDiagnosticSeverity = 'warning' | 'error'

export type SkillDiagnosticCode =
  | 'invalidRoot'
  | 'rootEscapesWorkspace'
  | 'tooManyEntries'
  | 'scanBudgetExceeded'
  | 'catalogTooLarge'
  | 'unreadableEntry'
  | 'unsupportedPathEncoding'
  | 'symlinkNotAllowed'
  | 'pathChangedDuringRead'
  | 'missingSkillFile'
  | 'skillFileTooLarge'
  | 'invalidUtf8'
  | 'nulByte'
  | 'missingFrontmatter'
  | 'invalidFrontmatter'
  | 'missingDescription'
  | 'invalidName'
  | 'invalidDescription'
  | 'invalidDirectoryName'
  | 'missingInstructions'
  | 'defaultedName'
  | 'duplicateName'
  | 'invalidInstallationReceipt'
  | 'packageRevisionMismatch'
  | 'unexpectedPackageEntry'
  | 'sourceUnavailable'
  | 'sourceContractViolation'

export interface SkillDescriptor {
  id: string
  name: string
  description: string
  source: SkillSourceDescriptor
  trust: SkillTrust
  activationScope: SkillActivationScope
  revision: string
  /** Human-readable source location. Never use this value to access the Skill. */
  location?: string
}

export interface SkillDiagnostic {
  code: SkillDiagnosticCode
  severity: SkillDiagnosticSeverity
  message: string
  skillId?: string
  /** Human-readable location only; it is not an authority-bearing path. */
  location?: string
}

export interface SkillsListInput {
  projectId?: string
}

export interface SkillsListOutput {
  schemaVersion: typeof SKILL_CATALOG_SCHEMA_VERSION
  catalogRevision: string
  skills: SkillDescriptor[]
  diagnostics: SkillDiagnostic[]
  truncated: boolean
}

export type SkillGitHubReference =
  { kind: 'defaultBranch' } | { kind: 'named'; value: string } | { kind: 'commit'; sha: string }

export type SkillAcquisitionSource =
  | { kind: 'localDirectory'; directory: string }
  | {
      kind: 'githubRepository'
      owner: string
      repository: string
      /** Moving reference to track. Omission means the repository default branch. */
      reference?: SkillGitHubReference
      /** Presentation pins must cross the one-time resolvedCandidate handoff instead. */
      resolvedCommit?: never
      subdirectory?: string
    }
  | {
      /** Opaque one-time authority returned by source resolution. */
      kind: 'resolvedCandidate'
      resolutionId: string
      candidateId: string
    }
  | { kind: 'installedSource' }

export type SkillInstallationIntent =
  | { operation: 'install' }
  | {
      operation: 'update'
      skillId: string
      expectedInstallationRevision: string
    }

export interface SkillsInspectInstallationInput {
  /**
   * Stable idempotency key for reservation, acquisition, and preview retries. For an install,
   * this UUID also becomes the new installation identity; `newInstallationIdentity` recovery
   * therefore requires a newly generated preparationId.
   */
  preparationId: string
  intent: SkillInstallationIntent
  source: SkillAcquisitionSource
}

export type SkillPreparationOperation = 'install' | 'update'

export interface SkillPackagePreview {
  formatVersion: number
  packageRevision: string
  name: string
  description: string
  fileCount: number
  totalBytes: number
}

/** A user-provided locator. Resolving it is read-only and never installs a Skill. */
export type SkillInstallationSourceLocator = { kind: 'url'; url: string }

export interface SkillsResolveInstallationSourceInput {
  /**
   * Client-generated canonical non-nil lowercase UUID used for idempotent retries of this exact
   * locator. After cancellation or abandonment, clients must generate a new resolutionId.
   */
  resolutionId: string
  locator: SkillInstallationSourceLocator
}

export interface SkillsCancelSourceResolutionInput {
  /** The client-generated identity of the resolution whose retained authority can be released. */
  resolutionId: string
}

/**
 * `alreadyAbsent` still establishes a bounded cancellation fence. Resolve and cancel share one
 * FIFO backend lane, so a resolve admitted before this cancellation cannot publish authority
 * after cancellation returns. Reusing a cancelled ID for a later resolve is unsupported; create
 * a new resolutionId instead.
 */
export type SkillSourceResolutionCancellationOutcome =
  'cancelled' | 'alreadyCancelled' | 'alreadyConsumed' | 'alreadyAbsent'

export interface SkillsCancelSourceResolutionOutput {
  schemaVersion: typeof SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION
  resolutionId: string
  outcome: SkillSourceResolutionCancellationOutcome
}

/**
 * Immutable presentation metadata returned by source resolution. Acquisition uses the
 * candidate's separate one-time `acquisition` handle.
 */
export interface SkillResolvedGitHubRepositorySource {
  kind: 'githubRepository'
  owner: string
  repository: string
  /** The branch, tag, default branch, or fixed commit this installation tracks. */
  reference: SkillGitHubReference
  /** The exact immutable snapshot inspected by source resolution. */
  resolvedCommit: string
  subdirectory?: string
}

export interface SkillSourceResolutionCandidate {
  /** Opaque stable identity. Clients must not derive meaning from this value. */
  candidateId: string
  /** One-time acquisition authority. Pass this exact value to skills.inspectInstallation. */
  acquisition: Extract<SkillAcquisitionSource, { kind: 'resolvedCandidate' }>
  /** Presentation and audit metadata; never use this field as acquisition authority. */
  source: SkillResolvedGitHubRepositorySource
  package: SkillPackagePreview
}

export type SkillSourceResolutionProvider = 'github'
export type SkillSourceResolutionOutcome = 'resolved' | 'selectionRequired'

interface SkillsResolveInstallationSourceOutputBase {
  schemaVersion: typeof SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION
  resolutionId: string
  canonicalUrl: string
  provider: SkillSourceResolutionProvider
  /** Canonical, lower-case, complete 40-character commit SHA. */
  resolvedCommit: string
  /** The one-time candidate authorities cannot be inspected after this Unix timestamp. */
  expiresAtUnixMs: number
}

export type SkillsResolveInstallationSourceOutput =
  | (SkillsResolveInstallationSourceOutputBase & {
      outcome: 'resolved'
      candidates: [SkillSourceResolutionCandidate]
    })
  | (SkillsResolveInstallationSourceOutputBase & {
      outcome: 'selectionRequired'
      candidates: [
        SkillSourceResolutionCandidate,
        SkillSourceResolutionCandidate,
        ...SkillSourceResolutionCandidate[]
      ]
    })

export type SkillSourceResolutionPhase = 'parse' | 'resolve' | 'discover'

export type SkillSourceResolutionErrorCode =
  | 'invalidLocator'
  | 'unsupportedLocator'
  | 'unsupportedHost'
  | 'unsupportedUrlShape'
  | 'repositoryNotFound'
  | 'referenceNotFound'
  | 'pathNotFound'
  | 'ambiguousReference'
  | 'noSkillsFound'
  | 'tooManySkills'
  | 'networkUnavailable'
  | 'rateLimited'
  | 'repositoryTooLarge'
  | 'unsafePackage'
  | 'invalidPackage'
  | 'resolutionIdConflict'
  | 'resolutionNotFoundOrExpired'
  | 'resolutionConsumed'
  | 'candidateNotFound'
  | 'capacityExceeded'
  | 'cancelled'
  | 'unavailable'

export type SkillSourceResolutionRecovery =
  | 'fixLocator'
  | 'retryLater'
  | 'narrowLocator'
  | 'chooseDifferentSource'
  | 'retrySameResolution'
  | 'startNewResolution'

export interface SkillSourceResolutionErrorData {
  type: 'skillSourceResolution'
  phase: SkillSourceResolutionPhase
  code: SkillSourceResolutionErrorCode
  recovery: SkillSourceResolutionRecovery
  message: string
  provider?: SkillSourceResolutionProvider
  retryAfterMs?: number
}

/** Presentation-safe source metadata; this union never carries authority-bearing local paths. */
export type SkillPreviewSource =
  | { kind: 'localDirectory'; displayName: string; refreshable: boolean }
  | {
      kind: 'githubRepository'
      owner: string
      repository: string
      reference: SkillGitHubReference
      resolvedCommit: string
      subdirectory?: string
      refreshable: boolean
    }
  | { kind: 'installedSource'; displayName: string; refreshable: boolean }

export type SkillCompatibilityStatus =
  'compatible' | 'compatibleWithWarnings' | 'unknown' | 'incompatible'

export type SkillCompatibilityIssueSeverity = 'warning' | 'error'

export interface SkillCompatibilityIssue {
  id: string
  /** Stable server identifier; clients must tolerate identifiers added later. */
  code: string
  severity: SkillCompatibilityIssueSeverity
  message: string
  capability?: string
  requiresAcknowledgement: boolean
}

export interface SkillCompatibilityReport {
  status: SkillCompatibilityStatus
  issues: SkillCompatibilityIssue[]
}

export type SkillInstallationChange = 'new' | 'changed' | 'unchanged'

export interface SkillInstallationChanges {
  content: SkillInstallationChange
  source: SkillInstallationChange
}

export interface SkillInstallationPreview {
  schemaVersion: typeof SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION
  preparationId: string
  previewRevision: string
  operation: SkillPreparationOperation
  installationId: string
  skillId: string
  package: SkillPackagePreview
  source: SkillPreviewSource
  compatibility: SkillCompatibilityReport
  changes: SkillInstallationChanges
  expiresAtUnixMs: number
}

export interface SkillsCommitInstallationInput {
  preparationId: string
  previewRevision: string
  acceptedIssueIds: string[]
}

export type SkillInstallationCommitOutcome =
  'installed' | 'alreadyInstalled' | 'updated' | 'alreadyCurrent'

export interface SkillInstallationCommitOutput {
  schemaVersion: typeof SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION
  preparationId: string
  operation: SkillPreparationOperation
  outcome: SkillInstallationCommitOutcome
  installationId: string
  skillId: string
  packageRevision: string
  installationRevision: string
  changes: SkillInstallationChanges
}

export interface SkillsCancelPreparationInput {
  preparationId: string
}

export type SkillPreparationCancellationOutcome = 'cancelled' | 'alreadyAbsent'

export interface SkillPreparationCancellationOutput {
  schemaVersion: typeof SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION
  preparationId: string
  outcome: SkillPreparationCancellationOutcome
}

export type SkillInspectionPhase = 'inspect' | 'commit' | 'cancel'

export type SkillInspectionErrorCode =
  | 'invalidSource'
  | 'unsupportedSource'
  | 'sourceNotAccessible'
  | 'referenceNotFound'
  | 'subdirectoryNotFound'
  | 'sourceChangedDuringRead'
  | 'resolutionNotFound'
  | 'resolutionExpired'
  | 'resolutionConsumed'
  | 'candidateNotFound'
  | 'capacityExceeded'
  | 'installationRetired'
  | 'installationNotFound'
  | 'installationRevisionConflict'
  | 'sourceNotRefreshable'
  | 'persistedSourceInvalid'
  | 'networkUnavailable'
  | 'rateLimited'
  | 'repositoryTooLarge'
  | 'unsafePackage'
  | 'invalidPackage'
  | 'incompatible'
  | 'preparationNotFound'
  | 'preparationExpired'
  | 'idempotencyConflict'
  | 'previewMismatch'
  | 'acknowledgementRequired'
  | 'commitIndeterminate'
  | 'cancelled'
  | 'unavailable'

export type SkillInspectionRecovery =
  | 'fixSource'
  | 'retrySamePreparation'
  | 'retryLater'
  | 'inspectAgain'
  | 'newInstallationIdentity'
  | 'freeCapacity'
  | 'contactSupport'
  | 'acknowledgeWarnings'
  | 'chooseDifferentSource'
  | 'resolveAgain'
  | 'refreshManagement'

export interface SkillInspectionErrorData {
  type: 'skillInspection'
  phase: SkillInspectionPhase
  code: SkillInspectionErrorCode
  recovery: SkillInspectionRecovery
  message: string
  preparationId?: string
  diagnosticCode?: string
  retryAfterMs?: number
  /** Present only when a commit failure may describe an already-visible mutation. */
  commitMayHaveSucceeded?: true
  skillId?: string
  intendedInstallationRevision?: string
}

export interface SkillsListManagementInput {
  projectId?: string
}

export type SkillManagementOperation = 'list' | 'setEnabled'

export type SkillManagementErrorCode =
  'notFound' | 'notManageable' | 'stateConflict' | 'storageUnavailable'

export type SkillManagementRecovery = 'refreshManagement' | 'retry'

export interface SkillManagementErrorData {
  type: 'skillManagement'
  operation: SkillManagementOperation
  code: SkillManagementErrorCode
  recovery: SkillManagementRecovery
  message: string
}

export interface SkillManagementActions {
  canSetEnabled: boolean
  canUpdate: boolean
  canUninstall: boolean
}

export interface SkillManagementEntry {
  id: string
  name: string
  description: string
  source: SkillSourceDescriptor
  packageRevision: string
  installationRevision?: string
  stateRevision: string
  enabled: boolean
  actions: SkillManagementActions
  acquisition?: SkillPreviewSource
  compatibility: SkillCompatibilityReport
}

export interface SkillsListManagementOutput {
  schemaVersion: typeof SKILL_MANAGEMENT_SCHEMA_VERSION
  managementRevision: string
  skills: SkillManagementEntry[]
  diagnostics: SkillDiagnostic[]
  truncated: boolean
}

export interface SkillsSetEnabledInput {
  skillId: string
  expectedStateRevision: string
  enabled: boolean
}

export type SkillSetEnabledOutcome = 'updated' | 'alreadyCurrent'

export interface SkillsSetEnabledOutput {
  schemaVersion: typeof SKILL_MANAGEMENT_SCHEMA_VERSION
  managementRevision: string
  skillId: string
  stateRevision: string
  enabled: boolean
  outcome: SkillSetEnabledOutcome
}

export type SkillsChangedReason =
  'installed' | 'updated' | 'uninstalled' | 'enablementChanged' | 'catalogChanged'

export interface SkillsChangedNotification {
  schemaVersion: typeof SKILL_MANAGEMENT_SCHEMA_VERSION
  managementRevision: string
  reason: SkillsChangedReason
  skillId?: string
}

export interface SkillsInstallLocalInput {
  /** Stable idempotency key. Reuse it when retrying the same installation. */
  installationId: string
  directory: string
}

export interface SkillsUpdateLocalInput {
  skillId: string
  expectedRevision: string
  directory: string
}

export interface SkillsUninstallInput {
  skillId: string
  /** Use the exact installationRevision returned by listManagement. */
  expectedRevision: string
}

interface SkillMutationOutputBase {
  schemaVersion: typeof SKILL_MUTATION_SCHEMA_VERSION
  installationId: string
  skillId: string
}

export type SkillMutationOutput = SkillMutationOutputBase &
  (
    | {
        outcome: 'installed' | 'alreadyInstalled' | 'updated' | 'alreadyCurrent'
        revision: string
      }
    | {
        outcome: 'uninstalled' | 'alreadyAbsent'
        revision?: never
      }
  )

export type SkillMutationOutcome = SkillMutationOutput['outcome']

export type SkillInstallationOperation = 'install' | 'update' | 'uninstall'

export type SkillInstallationErrorCode =
  | 'preparationFailed'
  | 'invalidSkill'
  | 'invalidStore'
  | 'capacityExceeded'
  | 'installationExists'
  | 'installationRetired'
  | 'installationNotFound'
  | 'revisionConflict'
  | 'storeCorrupt'
  | 'io'
  | 'unavailable'
  | 'commitIndeterminate'
  | 'cancelled'

export type SkillInstallationRecovery =
  | 'fixLocalSource'
  | 'retrySameRequest'
  | 'newInstallationIdentity'
  | 'refreshCatalog'
  | 'freeCapacity'
  | 'contactSupport'
  | 'repairStore'

export type SkillInstallationCapacity =
  'installations' | 'installationDirectory' | 'retiredInstallationIds' | 'packages'

export interface SkillInstallationErrorData {
  type: 'skillInstallation'
  operation: SkillInstallationOperation
  code: SkillInstallationErrorCode
  recovery: SkillInstallationRecovery
  message: string
  commitMayHaveSucceeded: boolean
  installationId?: string
  skillId?: string
  /** Stable server diagnostic identifier; clients must tolerate identifiers added later. */
  diagnosticCode?: string
  intendedRevision?: string
  expectedRevision?: string
  actualRevision?: string
  capacity?: SkillInstallationCapacity
  limit?: number
}

export interface ActivatedSkillSummary {
  id: string
  name: string
  revision: string
  source: SkillSourceDescriptor
}

export type SkillActivationErrorCode =
  | 'invalidSelection'
  | 'duplicateSelection'
  | 'tooManySkills'
  | 'activationTooLarge'
  | 'notFound'
  | 'stale'
  | 'invalidSkill'
  | 'disabled'
  | 'sourceUnavailable'

export interface SkillActivationErrorData {
  type: 'skillActivation'
  code: SkillActivationErrorCode
  recovery: SkillRecovery
  message: string
  skillId?: string
  expectedRevision?: string
  actualRevision?: string
}
