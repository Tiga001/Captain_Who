export type WorkflowErrorOperation = 'load' | 'save' | 'delete' | 'duplicate'

export interface WorkflowOperationError {
  operation: WorkflowErrorOperation
  kind:
    | 'configuration'
    | 'unavailable'
    | 'duplicate_member_name'
    | 'duplicate_department_name'
    | 'department_name_separator'
}

/** Turn an implementation error into a safe presentation category, never user-facing raw text. */
export function workflowOperationError(
  error: unknown,
  operation: WorkflowErrorOperation
): WorkflowOperationError {
  const detail = workflowErrorDetail(error)
  if (detail.includes('organization_duplicate_member_name'))
    return { operation, kind: 'duplicate_member_name' }
  if (detail.includes('organization_duplicate_department_name'))
    return { operation, kind: 'duplicate_department_name' }
  if (detail.includes('organization_department_name_separator'))
    return { operation, kind: 'department_name_separator' }
  const configuration = [
    /\b(?:Invalid organization|Unsupported organization version)\b/i,
    /\bInvalid (?:member rank|department name or bounds|node position|viewport)\b/i,
    /\b(?:Unknown (?:parent|member) department|Department hierarchy contains a cycle)\b/i,
    /\bDuplicate or invalid (?:node|department) identifiers\b/i,
    /\b(?:Organization(?: text)?|Node name) exceeds\b/i
  ].some((pattern) => pattern.test(detail))
  return { operation, kind: configuration ? 'configuration' : 'unavailable' }
}

/** Preserve diagnostics for internal matching only. Never render this string in product UI. */
export function workflowErrorDetail(error: unknown): string {
  const record =
    error && typeof error === 'object' ? (error as { message?: unknown; code?: unknown }) : null
  const message =
    typeof error === 'string' ? error : typeof record?.message === 'string' ? record.message : ''
  const normalized = Array.from(message)
    .filter((character) => {
      const code = character.charCodeAt(0)
      return (code >= 32 && code !== 127) || '\n\r\t'.includes(character)
    })
    .join('')
    .trim()
  const bounded = normalized.length > 1200 ? `${normalized.slice(0, 1200)}…` : normalized
  const code =
    typeof record?.code === 'number' && Number.isFinite(record.code) ? `[${record.code}]` : ''
  return [code, bounded].filter(Boolean).join(' ')
}
