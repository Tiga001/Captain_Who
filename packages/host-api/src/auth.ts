/** Cloud account DTOs. Session tokens and passwords must never be returned to Renderer. */
export interface AccountProfile {
  userId: string
  displayName: string
  email: string
  avatarDataUrl: string | null
  occupation: string
  organization: string
}

export type AuthErrorCode =
  | 'network'
  | 'credentials'
  | 'inactive'
  | 'profile'
  | 'storage'
  | 'code'
  | 'rateLimit'
  | 'verificationUnavailable'
  | 'expired'
  | 'unknown'

export interface AuthState {
  revision: number
  status: 'checking' | 'signedOut' | 'signedIn' | 'error'
  profile: AccountProfile | null
  error: AuthErrorCode | null
  /** False means this login is valid only for this app process. */
  remembered: boolean
}

export type AuthActionResult = { ok: true } | { ok: false; error: AuthErrorCode }

export interface AuthHostApi {
  getState(): Promise<AuthState>
  restoreSession(): Promise<AuthActionResult>
  login(input: { email: string; password: string }): Promise<AuthActionResult>
  sendEmailCode(email: string): Promise<AuthActionResult>
  verifyEmailCode(input: { email: string; code: string }): Promise<AuthActionResult>
  logout(): Promise<AuthActionResult>
  refreshProfile(): Promise<AuthActionResult>
  openWebsite(page: 'register' | 'reset' | 'profile'): Promise<void>
  onStateChanged(handler: (state: AuthState) => void): () => void
}
