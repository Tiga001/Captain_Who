export const SKILL_CATALOG_SCHEMA_VERSION = 4 as const
export const SKILL_MUTATION_SCHEMA_VERSION = 1 as const
export const SKILL_INSTALLATION_ERROR_CODE = -32010 as const

export const SKILLS_INSTALL_LOCAL_METHOD = 'skills.installLocal' as const
export const SKILLS_UPDATE_LOCAL_METHOD = 'skills.updateLocal' as const
export const SKILLS_UNINSTALL_METHOD = 'skills.uninstall' as const

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
  projectId: string
}

export interface SkillsListOutput {
  schemaVersion: typeof SKILL_CATALOG_SCHEMA_VERSION
  catalogRevision: string
  skills: SkillDescriptor[]
  diagnostics: SkillDiagnostic[]
  truncated: boolean
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
