export type LicenseReason = 'active' | 'expired' | 'revoked' | 'not_started'
export type LicenseError = 'network' | 'invalidResponse' | 'notProvisioned' | 'storage'

/** Main-owned license result only; contains no authentication credentials. */
export interface LicenseState {
  revision: number
  status: 'signedOut' | 'checking' | 'allowed' | 'denied' | 'unavailable'
  reason: LicenseReason | null
  expiresAt: string | null
  verifiedAt: string | null
  cacheValidUntil: string | null
  error: LicenseError | null
}

export interface LicenseHostApi {
  getState(): Promise<LicenseState>
  refresh(): Promise<LicenseState>
  openManagement(): Promise<void>
  onStateChanged(handler: (state: LicenseState) => void): () => void
}
