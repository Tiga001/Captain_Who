export const SKILL_CATALOG_SCHEMA_VERSION = 4 as const

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
