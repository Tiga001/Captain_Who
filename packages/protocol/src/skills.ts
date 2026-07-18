export const SKILL_CATALOG_SCHEMA_VERSION = 4 as const
export const SKILL_MUTATION_SCHEMA_VERSION = 1 as const
export const SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION = 1 as const
export const SKILL_MANAGEMENT_SCHEMA_VERSION = 1 as const
export const SKILL_INSTALLATION_ERROR_CODE = -32010 as const
export const SKILL_INSPECTION_ERROR_CODE = -32011 as const
export const SKILL_MANAGEMENT_ERROR_CODE = -32012 as const

export const SKILLS_INSTALL_LOCAL_METHOD = 'skills.installLocal' as const
export const SKILLS_UPDATE_LOCAL_METHOD = 'skills.updateLocal' as const
export const SKILLS_UNINSTALL_METHOD = 'skills.uninstall' as const
export const SKILLS_INSPECT_INSTALLATION_METHOD = 'skills.inspectInstallation' as const
export const SKILLS_COMMIT_INSTALLATION_METHOD = 'skills.commitInstallation' as const
export const SKILLS_CANCEL_PREPARATION_METHOD = 'skills.cancelPreparation' as const
export const SKILLS_LIST_MANAGEMENT_METHOD = 'skills.listManagement' as const
export const SKILLS_SET_ENABLED_METHOD = 'skills.setEnabled' as const
export const SKILLS_CHANGED_NOTIFICATION_METHOD = 'skills.changed' as const

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
      reference?: SkillGitHubReference
      subdirectory?: string
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
  /** Stable idempotency key for reservation, acquisition, and preview retries. */
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
  | 'cancelled'
  | 'unavailable'

export type SkillInspectionRecovery =
  | 'fixSource'
  | 'retrySamePreparation'
  | 'retryLater'
  | 'inspectAgain'
  | 'acknowledgeWarnings'
  | 'chooseDifferentSource'

export interface SkillInspectionErrorData {
  type: 'skillInspection'
  phase: SkillInspectionPhase
  code: SkillInspectionErrorCode
  recovery: SkillInspectionRecovery
  message: string
  preparationId?: string
  diagnosticCode?: string
  retryAfterMs?: number
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

const PACKAGE_MUTATION_OUTCOMES = [
  'installed',
  'alreadyInstalled',
  'updated',
  'alreadyCurrent'
] as const
const REMOVAL_MUTATION_OUTCOMES = ['uninstalled', 'alreadyAbsent'] as const

type SkillPackageMutationOutcome = (typeof PACKAGE_MUTATION_OUTCOMES)[number]
type SkillRemovalMutationOutcome = (typeof REMOVAL_MUTATION_OUTCOMES)[number]

/** Validates the untrusted JSON-RPC result before it enters typed application code. */
export function parseSkillMutationOutput(
  value: unknown,
  operation: SkillInstallationOperation
): SkillMutationOutput {
  if (!isRecord(value)) {
    throw invalidSkillMutationOutput('expected an object')
  }
  if (value.schemaVersion !== SKILL_MUTATION_SCHEMA_VERSION) {
    throw invalidSkillMutationOutput(`unsupported schema version ${String(value.schemaVersion)}`)
  }
  if (!isNonEmptyString(value.installationId)) {
    throw invalidSkillMutationOutput('installationId must be a non-empty string')
  }
  if (!isNonEmptyString(value.skillId)) {
    throw invalidSkillMutationOutput('skillId must be a non-empty string')
  }
  if (typeof value.outcome !== 'string') {
    throw invalidSkillMutationOutput('outcome must be a string')
  }

  const outcome = value.outcome
  if (isPackageMutationOutcome(outcome)) {
    if (!isOutcomeForOperation(operation, outcome)) {
      throw invalidSkillMutationOutput(`outcome ${outcome} is invalid for ${operation}`)
    }
    if (!isNonEmptyString(value.revision)) {
      throw invalidSkillMutationOutput(`outcome ${outcome} requires a non-empty revision`)
    }
    return {
      schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
      installationId: value.installationId,
      skillId: value.skillId,
      revision: value.revision,
      outcome
    }
  }
  if (isRemovalMutationOutcome(outcome)) {
    if (!isOutcomeForOperation(operation, outcome)) {
      throw invalidSkillMutationOutput(`outcome ${outcome} is invalid for ${operation}`)
    }
    if (Object.hasOwn(value, 'revision')) {
      throw invalidSkillMutationOutput(`outcome ${outcome} must not contain a revision`)
    }
    return {
      schemaVersion: SKILL_MUTATION_SCHEMA_VERSION,
      installationId: value.installationId,
      skillId: value.skillId,
      outcome
    }
  }
  throw invalidSkillMutationOutput(`unknown outcome ${value.outcome}`)
}

function isPackageMutationOutcome(value: string): value is SkillPackageMutationOutcome {
  return (PACKAGE_MUTATION_OUTCOMES as readonly string[]).includes(value)
}

function isRemovalMutationOutcome(value: string): value is SkillRemovalMutationOutcome {
  return (REMOVAL_MUTATION_OUTCOMES as readonly string[]).includes(value)
}

function isOutcomeForOperation(
  operation: SkillInstallationOperation,
  outcome: SkillMutationOutcome
): boolean {
  switch (operation) {
    case 'install':
      return outcome === 'installed' || outcome === 'alreadyInstalled'
    case 'update':
      return outcome === 'updated' || outcome === 'alreadyCurrent'
    case 'uninstall':
      return outcome === 'uninstalled' || outcome === 'alreadyAbsent'
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0
}

function invalidSkillMutationOutput(reason: string): Error {
  return new Error(`Invalid Skill mutation response: ${reason}`)
}

/** Validates a persisted installation preview before UI code may present or commit it. */
export function parseSkillInstallationPreview(value: unknown): SkillInstallationPreview {
  const record = expectRecord(value, 'Skill installation preview')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill installation preview'
  )
  const operation = expectEnum(
    record.operation,
    ['install', 'update'] as const,
    'Skill installation preview.operation'
  )
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill installation preview.preparationId'
    ),
    previewRevision: expectNonEmptyString(
      record.previewRevision,
      'Skill installation preview.previewRevision'
    ),
    operation,
    installationId: expectNonEmptyString(
      record.installationId,
      'Skill installation preview.installationId'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill installation preview.skillId'),
    package: parseSkillPackagePreview(record.package),
    source: parseSkillPreviewSource(record.source),
    compatibility: parseSkillCompatibilityReport(record.compatibility),
    changes: parseSkillInstallationChanges(record.changes),
    expiresAtUnixMs: expectSafeInteger(
      record.expiresAtUnixMs,
      'Skill installation preview.expiresAtUnixMs',
      1
    )
  }
}

/** Validates the exact result of committing one frozen installation preview. */
export function parseSkillInstallationCommitOutput(value: unknown): SkillInstallationCommitOutput {
  const record = expectRecord(value, 'Skill installation commit response')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill installation commit response'
  )
  const operation = expectEnum(
    record.operation,
    ['install', 'update'] as const,
    'Skill installation commit response.operation'
  )
  const outcome = expectEnum(
    record.outcome,
    ['installed', 'alreadyInstalled', 'updated', 'alreadyCurrent'] as const,
    'Skill installation commit response.outcome'
  )
  if (
    (operation === 'install' && outcome !== 'installed' && outcome !== 'alreadyInstalled') ||
    (operation === 'update' && outcome !== 'updated' && outcome !== 'alreadyCurrent')
  ) {
    throw invalidProtocolValue(
      'Skill installation commit response',
      `outcome ${outcome} is invalid for ${operation}`
    )
  }
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill installation commit response.preparationId'
    ),
    operation,
    outcome,
    installationId: expectNonEmptyString(
      record.installationId,
      'Skill installation commit response.installationId'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill installation commit response.skillId'),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill installation commit response.packageRevision'
    ),
    installationRevision: expectNonEmptyString(
      record.installationRevision,
      'Skill installation commit response.installationRevision'
    ),
    changes: parseSkillInstallationChanges(record.changes)
  }
}

export function parseSkillPreparationCancellationOutput(
  value: unknown
): SkillPreparationCancellationOutput {
  const record = expectRecord(value, 'Skill preparation cancellation response')
  expectSchemaVersion(
    record,
    SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    'Skill preparation cancellation response'
  )
  return {
    schemaVersion: SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION,
    preparationId: expectNonEmptyString(
      record.preparationId,
      'Skill preparation cancellation response.preparationId'
    ),
    outcome: expectEnum(
      record.outcome,
      ['cancelled', 'alreadyAbsent'] as const,
      'Skill preparation cancellation response.outcome'
    )
  }
}

export function parseSkillsListManagementOutput(value: unknown): SkillsListManagementOutput {
  const record = expectRecord(value, 'Skill management list response')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skill management list response')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skill management list response.managementRevision'
    ),
    skills: expectArray(record.skills, 'Skill management list response.skills').map(
      parseSkillManagementEntry
    ),
    diagnostics: expectArray(record.diagnostics, 'Skill management list response.diagnostics').map(
      parseSkillDiagnostic
    ),
    truncated: expectBoolean(record.truncated, 'Skill management list response.truncated')
  }
}

export function parseSkillsSetEnabledOutput(value: unknown): SkillsSetEnabledOutput {
  const record = expectRecord(value, 'Skill enablement response')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skill enablement response')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skill enablement response.managementRevision'
    ),
    skillId: expectNonEmptyString(record.skillId, 'Skill enablement response.skillId'),
    stateRevision: expectNonEmptyString(
      record.stateRevision,
      'Skill enablement response.stateRevision'
    ),
    enabled: expectBoolean(record.enabled, 'Skill enablement response.enabled'),
    outcome: expectEnum(
      record.outcome,
      ['updated', 'alreadyCurrent'] as const,
      'Skill enablement response.outcome'
    )
  }
}

export function parseSkillsChangedNotification(value: unknown): SkillsChangedNotification {
  const record = expectRecord(value, 'Skills changed notification')
  expectSchemaVersion(record, SKILL_MANAGEMENT_SCHEMA_VERSION, 'Skills changed notification')
  const skillId = optionalNonEmptyString(record.skillId, 'Skills changed notification.skillId')
  return {
    schemaVersion: SKILL_MANAGEMENT_SCHEMA_VERSION,
    managementRevision: expectNonEmptyString(
      record.managementRevision,
      'Skills changed notification.managementRevision'
    ),
    reason: expectEnum(
      record.reason,
      ['installed', 'updated', 'uninstalled', 'enablementChanged', 'catalogChanged'] as const,
      'Skills changed notification.reason'
    ),
    ...(skillId === undefined ? {} : { skillId })
  }
}

export function parseSkillInspectionErrorData(value: unknown): SkillInspectionErrorData {
  const record = expectRecord(value, 'Skill inspection error data')
  if (record.type !== 'skillInspection') {
    throw invalidProtocolValue('Skill inspection error data', 'type must be skillInspection')
  }
  const preparationId = optionalNonEmptyString(
    record.preparationId,
    'Skill inspection error data.preparationId'
  )
  const diagnosticCode = optionalNonEmptyString(
    record.diagnosticCode,
    'Skill inspection error data.diagnosticCode'
  )
  const retryAfterMs =
    record.retryAfterMs === undefined
      ? undefined
      : expectSafeInteger(record.retryAfterMs, 'Skill inspection error data.retryAfterMs', 0)
  return {
    type: 'skillInspection',
    phase: expectEnum(
      record.phase,
      ['inspect', 'commit', 'cancel'] as const,
      'Skill inspection error data.phase'
    ),
    code: expectEnum(
      record.code,
      [
        'invalidSource',
        'unsupportedSource',
        'sourceNotAccessible',
        'referenceNotFound',
        'subdirectoryNotFound',
        'sourceChangedDuringRead',
        'networkUnavailable',
        'rateLimited',
        'repositoryTooLarge',
        'unsafePackage',
        'invalidPackage',
        'incompatible',
        'preparationNotFound',
        'preparationExpired',
        'idempotencyConflict',
        'previewMismatch',
        'acknowledgementRequired',
        'cancelled',
        'unavailable'
      ] as const,
      'Skill inspection error data.code'
    ),
    recovery: expectEnum(
      record.recovery,
      [
        'fixSource',
        'retrySamePreparation',
        'retryLater',
        'inspectAgain',
        'acknowledgeWarnings',
        'chooseDifferentSource'
      ] as const,
      'Skill inspection error data.recovery'
    ),
    message: expectNonEmptyString(record.message, 'Skill inspection error data.message'),
    ...(preparationId === undefined ? {} : { preparationId }),
    ...(diagnosticCode === undefined ? {} : { diagnosticCode }),
    ...(retryAfterMs === undefined ? {} : { retryAfterMs })
  }
}

export function parseSkillManagementErrorData(value: unknown): SkillManagementErrorData {
  const record = expectRecord(value, 'Skill management error data')
  expectOnlyKeys(
    record,
    ['type', 'operation', 'code', 'recovery', 'message'] as const,
    'Skill management error data'
  )
  if (record.type !== 'skillManagement') {
    throw invalidProtocolValue('Skill management error data', 'type must be skillManagement')
  }
  return {
    type: 'skillManagement',
    operation: expectEnum(
      record.operation,
      ['list', 'setEnabled'] as const,
      'Skill management error data.operation'
    ),
    code: expectEnum(
      record.code,
      ['notFound', 'notManageable', 'stateConflict', 'storageUnavailable'] as const,
      'Skill management error data.code'
    ),
    recovery: expectEnum(
      record.recovery,
      ['refreshManagement', 'retry'] as const,
      'Skill management error data.recovery'
    ),
    message: expectNonEmptyString(record.message, 'Skill management error data.message')
  }
}

function parseSkillPackagePreview(value: unknown): SkillPackagePreview {
  const record = expectRecord(value, 'Skill package preview')
  return {
    formatVersion: expectSafeInteger(
      record.formatVersion,
      'Skill package preview.formatVersion',
      1
    ),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill package preview.packageRevision'
    ),
    name: expectNonEmptyString(record.name, 'Skill package preview.name'),
    description: expectNonEmptyString(record.description, 'Skill package preview.description'),
    fileCount: expectSafeInteger(record.fileCount, 'Skill package preview.fileCount', 1),
    totalBytes: expectSafeInteger(record.totalBytes, 'Skill package preview.totalBytes', 1)
  }
}

function parseSkillPreviewSource(value: unknown): SkillPreviewSource {
  const record = expectRecord(value, 'Skill preview source')
  switch (record.kind) {
    case 'localDirectory':
      return {
        kind: 'localDirectory',
        displayName: expectNonEmptyString(record.displayName, 'Skill preview source.displayName'),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    case 'githubRepository': {
      const subdirectory = optionalNonEmptyString(
        record.subdirectory,
        'Skill preview source.subdirectory'
      )
      return {
        kind: 'githubRepository',
        owner: expectNonEmptyString(record.owner, 'Skill preview source.owner'),
        repository: expectNonEmptyString(record.repository, 'Skill preview source.repository'),
        reference: parseSkillGitHubReference(record.reference),
        resolvedCommit: expectNonEmptyString(
          record.resolvedCommit,
          'Skill preview source.resolvedCommit'
        ),
        ...(subdirectory === undefined ? {} : { subdirectory }),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    }
    case 'installedSource':
      return {
        kind: 'installedSource',
        displayName: expectNonEmptyString(record.displayName, 'Skill preview source.displayName'),
        refreshable: expectBoolean(record.refreshable, 'Skill preview source.refreshable')
      }
    default:
      throw invalidProtocolValue('Skill preview source', `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillGitHubReference(value: unknown): SkillGitHubReference {
  const record = expectRecord(value, 'Skill GitHub reference')
  switch (record.kind) {
    case 'defaultBranch':
      return { kind: 'defaultBranch' }
    case 'named':
      return {
        kind: 'named',
        value: expectNonEmptyString(record.value, 'Skill GitHub reference.value')
      }
    case 'commit':
      return {
        kind: 'commit',
        sha: expectNonEmptyString(record.sha, 'Skill GitHub reference.sha')
      }
    default:
      throw invalidProtocolValue('Skill GitHub reference', `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillCompatibilityReport(value: unknown): SkillCompatibilityReport {
  const record = expectRecord(value, 'Skill compatibility report')
  return {
    status: expectEnum(
      record.status,
      ['compatible', 'compatibleWithWarnings', 'unknown', 'incompatible'] as const,
      'Skill compatibility report.status'
    ),
    issues: expectArray(record.issues, 'Skill compatibility report.issues').map(
      parseSkillCompatibilityIssue
    )
  }
}

function parseSkillCompatibilityIssue(value: unknown): SkillCompatibilityIssue {
  const record = expectRecord(value, 'Skill compatibility issue')
  const capability = optionalNonEmptyString(
    record.capability,
    'Skill compatibility issue.capability'
  )
  return {
    id: expectNonEmptyString(record.id, 'Skill compatibility issue.id'),
    code: expectNonEmptyString(record.code, 'Skill compatibility issue.code'),
    severity: expectEnum(
      record.severity,
      ['warning', 'error'] as const,
      'Skill compatibility issue.severity'
    ),
    message: expectNonEmptyString(record.message, 'Skill compatibility issue.message'),
    ...(capability === undefined ? {} : { capability }),
    requiresAcknowledgement: expectBoolean(
      record.requiresAcknowledgement,
      'Skill compatibility issue.requiresAcknowledgement'
    )
  }
}

function parseSkillInstallationChanges(value: unknown): SkillInstallationChanges {
  const record = expectRecord(value, 'Skill installation changes')
  return {
    content: expectEnum(
      record.content,
      ['new', 'changed', 'unchanged'] as const,
      'Skill installation changes.content'
    ),
    source: expectEnum(
      record.source,
      ['new', 'changed', 'unchanged'] as const,
      'Skill installation changes.source'
    )
  }
}

function parseSkillManagementEntry(value: unknown): SkillManagementEntry {
  const record = expectRecord(value, 'Skill management entry')
  const installationRevision = optionalNonEmptyString(
    record.installationRevision,
    'Skill management entry.installationRevision'
  )
  const acquisition =
    record.acquisition === undefined ? undefined : parseSkillPreviewSource(record.acquisition)
  return {
    id: expectNonEmptyString(record.id, 'Skill management entry.id'),
    name: expectNonEmptyString(record.name, 'Skill management entry.name'),
    description: expectNonEmptyString(record.description, 'Skill management entry.description'),
    source: parseSkillSourceDescriptor(record.source),
    packageRevision: expectNonEmptyString(
      record.packageRevision,
      'Skill management entry.packageRevision'
    ),
    ...(installationRevision === undefined ? {} : { installationRevision }),
    stateRevision: expectNonEmptyString(
      record.stateRevision,
      'Skill management entry.stateRevision'
    ),
    enabled: expectBoolean(record.enabled, 'Skill management entry.enabled'),
    actions: parseSkillManagementActions(record.actions),
    ...(acquisition === undefined ? {} : { acquisition }),
    compatibility: parseSkillCompatibilityReport(record.compatibility)
  }
}

function parseSkillManagementActions(value: unknown): SkillManagementActions {
  const record = expectRecord(value, 'Skill management actions')
  return {
    canSetEnabled: expectBoolean(record.canSetEnabled, 'Skill management actions.canSetEnabled'),
    canUpdate: expectBoolean(record.canUpdate, 'Skill management actions.canUpdate'),
    canUninstall: expectBoolean(record.canUninstall, 'Skill management actions.canUninstall')
  }
}

function parseSkillSourceDescriptor(value: unknown): SkillSourceDescriptor {
  const record = expectRecord(value, 'Skill source descriptor')
  const id = expectNonEmptyString(record.id, 'Skill source descriptor.id')
  switch (record.kind) {
    case 'workspace':
      return { kind: 'workspace', id }
    case 'bundled':
      return { kind: 'bundled', id }
    case 'installed':
      return { kind: 'installed', id }
    default:
      throw invalidProtocolValue('Skill source descriptor', `unknown kind ${String(record.kind)}`)
  }
}

function parseSkillDiagnostic(value: unknown): SkillDiagnostic {
  const record = expectRecord(value, 'Skill diagnostic')
  const skillId = optionalNonEmptyString(record.skillId, 'Skill diagnostic.skillId')
  const location = optionalNonEmptyString(record.location, 'Skill diagnostic.location')
  return {
    code: expectNonEmptyString(record.code, 'Skill diagnostic.code') as SkillDiagnosticCode,
    severity: expectEnum(
      record.severity,
      ['warning', 'error'] as const,
      'Skill diagnostic.severity'
    ),
    message: expectNonEmptyString(record.message, 'Skill diagnostic.message'),
    ...(skillId === undefined ? {} : { skillId }),
    ...(location === undefined ? {} : { location })
  }
}

function expectRecord(value: unknown, context: string): Record<string, unknown> {
  if (!isRecord(value)) throw invalidProtocolValue(context, 'expected an object')
  return value
}

function expectOnlyKeys(
  record: Record<string, unknown>,
  allowedKeys: readonly string[],
  context: string
): void {
  const unexpectedKey = Object.keys(record).find((key) => !allowedKeys.includes(key))
  if (unexpectedKey) {
    throw invalidProtocolValue(context, `unexpected field ${unexpectedKey}`)
  }
}

function expectArray(value: unknown, context: string): unknown[] {
  if (!Array.isArray(value)) throw invalidProtocolValue(context, 'expected an array')
  return value
}

function expectNonEmptyString(value: unknown, context: string): string {
  if (!isNonEmptyString(value)) throw invalidProtocolValue(context, 'expected a non-empty string')
  return value
}

function optionalNonEmptyString(value: unknown, context: string): string | undefined {
  if (value === undefined) return undefined
  return expectNonEmptyString(value, context)
}

function expectBoolean(value: unknown, context: string): boolean {
  if (typeof value !== 'boolean') throw invalidProtocolValue(context, 'expected a boolean')
  return value
}

function expectSafeInteger(value: unknown, context: string, minimum: number): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum) {
    throw invalidProtocolValue(
      context,
      `expected a safe integer greater than or equal to ${minimum}`
    )
  }
  return value
}

function expectEnum<const T extends readonly string[]>(
  value: unknown,
  values: T,
  context: string
): T[number] {
  if (typeof value !== 'string' || !(values as readonly string[]).includes(value)) {
    throw invalidProtocolValue(context, `unexpected value ${String(value)}`)
  }
  return value as T[number]
}

function expectSchemaVersion(
  record: Record<string, unknown>,
  expected: number,
  context: string
): void {
  if (record.schemaVersion !== expected) {
    throw invalidProtocolValue(
      context,
      `unsupported schema version ${String(record.schemaVersion)}`
    )
  }
}

function invalidProtocolValue(context: string, reason: string): Error {
  return new Error(`Invalid ${context}: ${reason}`)
}

export type SkillInstallationOperation = 'install' | 'update' | 'uninstall'

export type SkillInstallationErrorCode =
  | 'preparationFailed'
  | 'invalidSkill'
  | 'invalidStore'
  | 'capacityExceeded'
  | 'installationExists'
  | 'installationNotFound'
  | 'revisionConflict'
  | 'storeCorrupt'
  | 'io'
  | 'unavailable'
  | 'commitIndeterminate'
  | 'cancelled'

export type SkillInstallationRecovery =
  'fixLocalSource' | 'retrySameRequest' | 'refreshCatalog' | 'freeCapacity' | 'repairStore'

export type SkillInstallationCapacity = 'installations' | 'installationDirectory' | 'packages'

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
