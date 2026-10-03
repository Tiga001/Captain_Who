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

export interface WorkflowUnavailableModel {
  modelConfigId: string
  modelDisplayName: string | null
  memberNames: string[]
}

/** Read only the explicit public error contract; unrelated failures must stay generic. */
export function workflowUnavailableMemberModels(error: unknown): WorkflowUnavailableModel[] | null {
  const isRecord = (value: unknown): value is Record<string, unknown> =>
    value !== null && typeof value === 'object' && !Array.isArray(value)
  const isText = (value: unknown): value is string =>
    typeof value === 'string' && value.trim().length > 0
  const code = 'organization_member_models_unavailable'
  if (
    !isRecord(error) ||
    error.code !== -32602 ||
    error.message !== code ||
    !isRecord(error.data) ||
    error.data.code !== code ||
    !Array.isArray(error.data.members) ||
    error.data.members.length === 0 ||
    error.data.members.length > 128
  )
    return null

  const groups = new Map<string, WorkflowUnavailableModel>()
  const seenMembers = new Map<string, Set<string>>()
  for (const member of error.data.members) {
    if (
      !isRecord(member) ||
      !isText(member.nodeId) ||
      !isText(member.nodeName) ||
      !isText(member.modelConfigId) ||
      (member.modelDisplayName !== null && typeof member.modelDisplayName !== 'string')
    )
      return null
    const name = member.modelDisplayName?.trim().replace(/\s+/g, ' ') || null
    let group = groups.get(member.modelConfigId)
    if (!group) {
      group = { modelConfigId: member.modelConfigId, modelDisplayName: name, memberNames: [] }
      groups.set(member.modelConfigId, group)
      seenMembers.set(member.modelConfigId, new Set())
    } else if (!group.modelDisplayName) group.modelDisplayName = name
    const seen = seenMembers.get(member.modelConfigId)!
    if (!seen.has(member.nodeId)) {
      group.memberNames.push(member.nodeName.trim().replace(/\s+/g, ' '))
      seen.add(member.nodeId)
    }
  }
  return [...groups.values()]
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
