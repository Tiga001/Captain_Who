// Renderer skills management errors: tolerantly classifies structured protocol failures for UI.
import { HostInvocationError } from '@mycopilot/host-api'
import type { TranslationKey } from '../../../config/languageRegistry'

export type SkillOperationErrorKind =
  'sourceResolution' | 'inspection' | 'management' | 'installation' | 'unknown'

export interface SkillOperationErrorDetails {
  code?: string
  commitMayHaveSucceeded: boolean
  kind: SkillOperationErrorKind
  phase?: string
  recovery?: string
  intendedInstallationRevision?: string
  skillId?: string
}

export function getSkillOperationErrorDetails(error: unknown): SkillOperationErrorDetails {
  if (!(error instanceof HostInvocationError) || !isRecord(error.data)) {
    return { commitMayHaveSucceeded: false, kind: 'unknown' }
  }

  const data = error.data
  const code = typeof data.code === 'string' ? data.code : undefined
  const recovery = typeof data.recovery === 'string' ? data.recovery : undefined
  const commitMayHaveSucceeded = data.commitMayHaveSucceeded === true
  const phase = typeof data.phase === 'string' ? data.phase : undefined
  const skillId = typeof data.skillId === 'string' ? data.skillId : undefined
  const intendedInstallationRevision =
    typeof data.intendedInstallationRevision === 'string'
      ? data.intendedInstallationRevision
      : undefined

  const shared = {
    code,
    commitMayHaveSucceeded,
    intendedInstallationRevision,
    phase,
    recovery,
    skillId
  }

  if (data.type === 'skillSourceResolution') {
    return { ...shared, kind: 'sourceResolution' }
  }

  if (data.type === 'skillInspection') {
    return { ...shared, kind: 'inspection' }
  }
  if (data.type === 'skillManagement') {
    return { ...shared, kind: 'management' }
  }
  if (data.type === 'skillInstallation') {
    return { ...shared, kind: 'installation' }
  }

  return { commitMayHaveSucceeded, kind: 'unknown' }
}

export function getSkillOperationErrorKey(
  details: SkillOperationErrorDetails,
  fallbackKey: TranslationKey = 'skills.operationFailed'
): TranslationKey {
  if (details.commitMayHaveSucceeded || details.code === 'commitIndeterminate') {
    return 'skills.operationNeedsConfirmation'
  }

  if (details.recovery === 'freeCapacity' || details.code === 'capacityExceeded') {
    return 'skills.freeCapacityDescription'
  }
  if (
    details.recovery === 'contactSupport' ||
    details.recovery === 'repairStore' ||
    details.code === 'storageUnavailable' ||
    details.code === 'storeCorrupt' ||
    details.code === 'invalidStore'
  ) {
    return 'skills.contactSupportDescription'
  }
  if (details.recovery === 'acknowledgeWarnings' || details.code === 'acknowledgementRequired') {
    return 'skills.error.acknowledgementRequired'
  }

  switch (details.code) {
    case 'networkUnavailable':
      return 'skills.error.networkUnavailable'
    case 'rateLimited':
      return 'skills.error.rateLimited'
    case 'repositoryTooLarge':
      return 'skills.error.repositoryTooLarge'
    case 'unsafePackage':
      return 'skills.error.unsafePackage'
    case 'invalidPackage':
    case 'invalidSkill':
      return 'skills.error.invalidPackage'
    case 'invalidLocator':
    case 'unsupportedLocator':
    case 'unsupportedUrlShape':
    case 'invalidSource':
    case 'unsupportedSource':
      return 'skills.error.invalidSource'
    case 'unsupportedHost':
      return 'skills.error.unsupportedHost'
    case 'repositoryNotFound':
    case 'referenceNotFound':
    case 'pathNotFound':
    case 'subdirectoryNotFound':
      return 'skills.error.sourceNotFound'
    case 'sourceNotAccessible':
      return 'skills.error.sourceNotAccessible'
    case 'noSkillsFound':
      return 'skills.error.noSkillsFound'
    case 'tooManySkills':
      return 'skills.error.tooManySkills'
    case 'incompatible':
      return 'skills.error.incompatible'
    case 'sourceChangedDuringRead':
      return 'skills.error.sourceChanged'
    case 'preparationExpired':
    case 'preparationNotFound':
    case 'resolutionExpired':
    case 'resolutionNotFound':
    case 'resolutionNotFoundOrExpired':
    case 'previewMismatch':
      return 'skills.previewExpiredDescription'
    case 'resolutionConsumed':
    case 'candidateNotFound':
      return 'skills.error.sourceAuthorityExpired'
    case 'stateConflict':
    case 'revisionConflict':
    case 'installationRevisionConflict':
    case 'installationNotFound':
    case 'installationRetired':
    case 'notFound':
      return 'skills.stateChanged'
    case 'notManageable':
      return 'skills.error.notManageable'
    case 'idempotencyConflict':
    case 'resolutionIdConflict':
    case 'installationExists':
      return 'skills.error.operationConflict'
    case 'cancelled':
      return 'skills.error.cancelled'
    case 'unavailable':
    case 'io':
      return 'skills.error.unavailable'
    default:
      return fallbackKey
  }
}

export function shouldRefreshSkillsAfterError(details: SkillOperationErrorDetails): boolean {
  return (
    details.commitMayHaveSucceeded ||
    details.code === 'stateConflict' ||
    details.code === 'revisionConflict' ||
    details.code === 'installationRevisionConflict' ||
    details.code === 'installationNotFound' ||
    details.code === 'installationRetired' ||
    details.code === 'notFound'
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}
