export type SkillScope = 'workspace'

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

export interface SkillDescriptor {
  id: string
  name: string
  description: string
  scope: SkillScope
  path: string
  relativePath: string
  revision: string
}

export interface SkillDiagnostic {
  code: SkillDiagnosticCode
  severity: SkillDiagnosticSeverity
  message: string
  path: string
}

export interface SkillsListInput {
  projectId: string
}

export interface SkillsListOutput {
  catalogRevision: string
  skills: SkillDescriptor[]
  diagnostics: SkillDiagnostic[]
  truncated: boolean
}
