import type { StorageProjectValidationErrorData } from '@mycopilot/protocol'

/** Thrown when Main rejects a project create/update with a renderer-localizable code. */
export class ProjectValidationError extends Error {
  readonly data: StorageProjectValidationErrorData

  constructor(data: StorageProjectValidationErrorData) {
    super(`Project validation failed: ${data.code}`)
    this.name = 'ProjectValidationError'
    this.data = data
  }
}

export function isProjectValidationErrorData(
  value: unknown
): value is StorageProjectValidationErrorData {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { kind?: unknown }).kind === 'project_validation' &&
    typeof (value as { code?: unknown }).code === 'string'
  )
}
