import {
  SKILL_MUTATION_SCHEMA_VERSION,
  type SkillInstallationOperation,
  type SkillMutationOutcome,
  type SkillMutationOutput
} from './contracts'
import { isNonEmptyString, isRecord } from './validation'

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

function invalidSkillMutationOutput(reason: string): Error {
  return new Error(`Invalid Skill mutation response: ${reason}`)
}
