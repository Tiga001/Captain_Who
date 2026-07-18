// Renderer skills management errors: tolerantly classifies structured protocol failures for UI.
import { HostInvocationError } from '@mycopilot/host-api'

export type SkillOperationErrorKind = 'inspection' | 'management' | 'installation' | 'unknown'

export interface SkillOperationErrorDetails {
  code?: string
  commitMayHaveSucceeded: boolean
  kind: SkillOperationErrorKind
  message: string
  recovery?: string
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

  if (data.type === 'skillInspection') {
    return {
      code,
      commitMayHaveSucceeded,
      kind: 'inspection',
      message: structuredMessage,
      recovery
    }
  }
  if (data.type === 'skillManagement') {
    return {
      code,
      commitMayHaveSucceeded,
      kind: 'management',
      message: structuredMessage,
      recovery
    }
  }
  if (data.type === 'skillInstallation') {
    return {
      code,
      commitMayHaveSucceeded,
      kind: 'installation',
      message: structuredMessage,
      recovery
    }
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
