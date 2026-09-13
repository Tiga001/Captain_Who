import type { LicenseError, LicenseReason } from '@mycopilot/host-api'
import { ACCOUNT_CONFIG } from './accountConfig'

export const LICENSE_CACHE_MS = 24 * 60 * 60_000
export interface VerifiedLicense {
  allowed: boolean
  reason: LicenseReason
  expiresAt: string | null
  verifiedAt: string
  cacheValidUntil: string
}
export class LicenseFailure extends Error {
  constructor(
    readonly code: LicenseError,
    readonly unauthorized = false
  ) {
    super(code)
  }
}

export function parseVerifiedLicense(value: unknown): VerifiedLicense {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new LicenseFailure('invalidResponse')
  const v = value as Record<string, unknown>
  const isTime = (x: unknown): x is string =>
    typeof x === 'string' &&
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/.test(x) &&
    Number.isFinite(Date.parse(x))
  if (
    typeof v.allowed !== 'boolean' ||
    !['active', 'expired', 'revoked', 'not_started'].includes(String(v.reason)) ||
    (v.expiresAt !== null && !isTime(v.expiresAt)) ||
    !isTime(v.verifiedAt) ||
    !isTime(v.cacheValidUntil)
  )
    throw new LicenseFailure('invalidResponse')
  const checked = Date.parse(v.verifiedAt)
  const deadline = Date.parse(v.cacheValidUntil)
  if (
    v.allowed !== (v.reason === 'active') ||
    (v.allowed &&
      (deadline <= checked ||
        deadline - checked > LICENSE_CACHE_MS ||
        (v.expiresAt !== null && deadline > Date.parse(v.expiresAt as string)))) ||
    (!v.allowed && deadline > checked)
  )
    throw new LicenseFailure('invalidResponse')
  return {
    allowed: v.allowed,
    reason: v.reason as LicenseReason,
    expiresAt: v.expiresAt as string | null,
    verifiedAt: v.verifiedAt,
    cacheValidUntil: v.cacheValidUntil
  }
}

export async function fetchAccountLicense(
  accessToken: string,
  signal: AbortSignal
): Promise<VerifiedLicense> {
  let response: Response
  try {
    response = await fetch(`${ACCOUNT_CONFIG.api}/v1/me/license`, {
      headers: { Authorization: `Bearer ${accessToken}`, Accept: 'application/json' },
      signal: AbortSignal.any([signal, AbortSignal.timeout(15_000)]),
      redirect: 'error',
      cache: 'no-store'
    })
  } catch {
    throw new LicenseFailure('network')
  }
  if (response.status === 401 || response.status === 403) throw new LicenseFailure('network', true)
  if (response.status !== 200) {
    let code: unknown
    try {
      code = (await response.json())?.error?.code
    } catch {
      /* Safe error only. */
    }
    throw new LicenseFailure(code === 'LICENSE_NOT_PROVISIONED' ? 'notProvisioned' : 'network')
  }
  try {
    return parseVerifiedLicense((await response.json())?.data)
  } catch (error) {
    if (error instanceof LicenseFailure) throw error
    throw new LicenseFailure('invalidResponse')
  }
}
