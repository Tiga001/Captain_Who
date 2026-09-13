/** Private Host → Core control plane. Never expose this method through Renderer IPC. */
export const CORE_SET_EXECUTION_ACCESS_METHOD = 'core.setExecutionAccess'
export const EXECUTION_ACCESS_MAX_TTL_MS = 60_000
export type ExecutionAccessReason =
  'allowed' | 'account_signed_out' | 'license_required' | 'license_unavailable'

export interface ExecutionAccessSnapshot {
  revision: number
  identityEpoch: number
  reason: ExecutionAccessReason
  issuedAt: number
  validUntil: number
}

export function parseExecutionAccessSnapshot(value: unknown): ExecutionAccessSnapshot {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid execution access snapshot')
  const v = value as Record<string, unknown>
  const integer = (n: unknown): n is number => typeof n === 'number' && Number.isSafeInteger(n)
  if (
    Object.keys(v).some(
      (key) => !['revision', 'identityEpoch', 'reason', 'issuedAt', 'validUntil'].includes(key)
    ) ||
    !integer(v.revision) ||
    v.revision < 1 ||
    !integer(v.identityEpoch) ||
    v.identityEpoch < 0 ||
    !integer(v.issuedAt) ||
    v.issuedAt < 0 ||
    !integer(v.validUntil) ||
    !['allowed', 'account_signed_out', 'license_required', 'license_unavailable'].includes(
      String(v.reason)
    ) ||
    v.validUntil < v.issuedAt ||
    v.validUntil - v.issuedAt > EXECUTION_ACCESS_MAX_TTL_MS ||
    (v.reason === 'allowed' ? v.validUntil === v.issuedAt : v.validUntil !== v.issuedAt)
  )
    throw new Error('Invalid execution access snapshot')
  return {
    revision: v.revision,
    identityEpoch: v.identityEpoch,
    reason: v.reason as ExecutionAccessReason,
    issuedAt: v.issuedAt,
    validUntil: v.validUntil
  }
}

export function parseExecutionAccessReceipt(value: unknown): { revision: number } {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((key) => key !== 'revision') ||
    !('revision' in value) ||
    typeof value.revision !== 'number' ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 1
  )
    throw new Error('Invalid execution access receipt')
  return { revision: value.revision }
}
