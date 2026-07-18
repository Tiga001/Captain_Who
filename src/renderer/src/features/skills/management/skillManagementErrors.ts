// Renderer skills management errors: tolerantly classifies structured protocol failures for UI.
import { HostInvocationError } from '@mycopilot/host-api'

export type SkillOperationErrorKind =
  'sourceResolution' | 'inspection' | 'management' | 'installation' | 'unknown'

export interface SkillOperationErrorDetails {
  code?: string
  commitMayHaveSucceeded: boolean
  kind: SkillOperationErrorKind
  message: string
  phase?: string
  recovery?: string
  intendedInstallationRevision?: string
  skillId?: string
}

export function getSkillOperationErrorDetails(error: unknown): SkillOperationErrorDetails {
  const message = error instanceof Error ? error.message : String(error)
  if (!(error instanceof HostInvocationError) || !isRecord(error.data)) {
    return { commitMayHaveSucceeded: false, kind: 'unknown', message }
  }

  const data = error.data
  const code = typeof data.code === 'string' ? data.code : undefined
  const recovery = typeof data.recovery === 'string' ? data.recovery : undefined
  const structuredMessage = typeof data.message === 'string' ? data.message : message
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
    message: structuredMessage,
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

  return { commitMayHaveSucceeded, kind: 'unknown', message }
}

export function shouldRefreshSkillsAfterError(details: SkillOperationErrorDetails): boolean {
  return (
    details.commitMayHaveSucceeded ||
    details.code === 'stateConflict' ||
    details.code === 'revisionConflict' ||
    details.code === 'installationNotFound' ||
    details.code === 'notFound'
  )
}

export function isExpiredSkillPreviewError(details: SkillOperationErrorDetails): boolean {
  return (
    details.code === 'preparationExpired' ||
    details.code === 'preparationNotFound' ||
    details.code === 'previewMismatch'
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}
