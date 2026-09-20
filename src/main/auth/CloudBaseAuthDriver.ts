import cloudbase from '@cloudbase/js-sdk'
import type { SignInRes } from '@cloudbase/js-sdk/auth'
import type { AuthOptions } from '@cloudbase/js-sdk/oauth'
import type { AuthErrorCode } from '@mycopilot/host-api'
import { ACCOUNT_CONFIG } from './accountConfig'
import type { SessionTokens } from './SessionStore'

export class AuthFailure extends Error {
  constructor(readonly code: AuthErrorCode) {
    super(code)
  }
}

export interface CloudSession extends SessionTokens {
  email: string
}

export interface AuthDriver {
  login(email: string, password: string): Promise<CloudSession>
  restore(tokens: SessionTokens): Promise<CloudSession>
  refresh(tokens: SessionTokens): Promise<CloudSession>
  sendCode(email: string): Promise<void>
  verifyCode(email: string, code: string): Promise<CloudSession>
  logout(): Promise<void>
}

export function sdkFailure(error: unknown, fallback: AuthErrorCode): AuthFailure {
  const value = error as { code?: unknown; errorCode?: unknown; message?: unknown }
  const codes = [value?.code, value?.errorCode]
    .filter((code): code is string | number => typeof code === 'string' || typeof code === 'number')
    .map((code) => String(code).trim().toLowerCase())
  const numericCodes = codes.map(Number)
  const message = typeof value?.message === 'string' ? value.message : ''
  // SDK refresh can turn invalid_grant into unauthenticated without a numeric errorCode.
  // Inspect both fields: a generic numeric code must not hide a terminal account error.
  if (codes.includes('user_blocked')) return new AuthFailure('inactive')
  if (
    codes.some((code) => code === 'unauthenticated' || code === 'invalid_grant') ||
    numericCodes.some((code) => code === 16 || code === 401)
  ) {
    return new AuthFailure('expired')
  }
  if (message.includes('CAPTAIN_WHO_INTERACTIVE_VERIFICATION_REQUIRED')) {
    return new AuthFailure('verificationUnavailable')
  }
  if (numericCodes.some((code) => code === 8 || code === 429)) return new AuthFailure('rateLimit')
  if (numericCodes.some((code) => [4001, 4002, 4042, 4045, 4022, 12].includes(code))) {
    return new AuthFailure('verificationUnavailable')
  }
  // CloudBase can return UNKNOWN for either invalid credentials or an unavailable
  // password-login provider. Keep the latter from being presented as a wrong password.
  if (
    /invalid[_\s-]?password|invalid credentials|incorrect password|user not found|账号或密码|邮箱或密码/i.test(
      message
    )
  ) {
    return new AuthFailure('credentials')
  }
  if (
    /用户名密码登录|username.?password|password.?login|signInWithPassword/i.test(message) &&
    /需确保|ensure|enable|开启|enabled|配置|configuration/i.test(message)
  ) {
    return new AuthFailure('unknown')
  }
  if (numericCodes.some((code) => [3, 5, 7].includes(code))) return new AuthFailure(fallback)
  // Inspect internally, but never forward raw SDK errors (which can include request details).
  if (
    /network|fetch|timeout|timed out|ECONN|ENOTFOUND|socket/i.test(message) ||
    numericCodes.includes(14)
  ) {
    return new AuthFailure('network')
  }
  return new AuthFailure(fallback)
}

// The Node adapter otherwise waits forever for a browser captcha callback which this desktop
// password/OTP flow does not provide. Report the requirement, never bypass the cloud challenge.
const DESKTOP_AUTH_OPTIONS: Pick<AuthOptions, 'captchaOptions'> & { persistence: 'none' } = {
  persistence: 'none',
  captchaOptions: {
    openURIWithCallback: async () => {
      throw Object.assign(new Error('CAPTAIN_WHO_INTERACTIVE_VERIFICATION_REQUIRED'), {
        errorCode: 4001
      })
    }
  }
}

export class CloudBaseAuthDriver implements AuthDriver {
  private readonly auth = cloudbase
    .init({
      env: ACCOUNT_CONFIG.env,
      region: ACCOUNT_CONFIG.region,
      accessKey: ACCOUNT_CONFIG.publishableKey,
      persistence: 'none',
      timeout: 15_000,
      debug: false,
      auth: { detectSessionInUrl: false }
    })
    .auth(DESKTOP_AUTH_OPTIONS)

  private challenge: {
    email: string
    expiresAt: number
    verify: (input: { token: string }) => Promise<SignInRes>
  } | null = null

  private async request<T>(operation: () => Promise<T>, fallback: AuthErrorCode): Promise<T> {
    try {
      return await operation()
    } catch (error) {
      throw sdkFailure(error, fallback)
    }
  }

  private async session(
    operation: () => Promise<SignInRes>,
    fallback: AuthErrorCode
  ): Promise<CloudSession> {
    const result = await this.request(operation, fallback)
    if (result.error) throw sdkFailure(result.error, fallback)
    const session = result.data?.session
    if (!session?.access_token || !session.refresh_token) throw new AuthFailure('expired')
    // Fetch the authenticated identity after restoration; the publishable anonymous identity is
    // never a user session, and email must come from CloudBase rather than the account profile.
    const userResult = await this.request(() => this.auth.getUser(), 'network')
    if (userResult.error) throw sdkFailure(userResult.error, 'network')
    const user = userResult.data?.user
    if (!user || user.is_anonymous || typeof user.email !== 'string' || !user.email) {
      throw new AuthFailure('profile')
    }
    return {
      access_token: session.access_token,
      refresh_token: session.refresh_token,
      email: user.email
    }
  }

  async login(email: string, password: string): Promise<CloudSession> {
    this.challenge = null
    return this.session(() => this.auth.signInWithPassword({ email, password }), 'credentials')
  }

  async restore(tokens: SessionTokens): Promise<CloudSession> {
    return this.session(() => this.auth.setSession(tokens), 'network')
  }

  async refresh(tokens: SessionTokens): Promise<CloudSession> {
    return this.session(() => this.auth.refreshSession(tokens.refresh_token), 'network')
  }

  async sendCode(email: string): Promise<void> {
    this.challenge = null
    const result = await this.request(
      () => this.auth.signInWithOtp({ email, options: { shouldCreateUser: false } }),
      'verificationUnavailable'
    )
    if (result.error) throw sdkFailure(result.error, 'verificationUnavailable')
    if (!result.data?.verifyOtp) throw new AuthFailure('verificationUnavailable')
    this.challenge = { email, expiresAt: Date.now() + 10 * 60_000, verify: result.data.verifyOtp }
  }

  async verifyCode(email: string, code: string): Promise<CloudSession> {
    const challenge = this.challenge
    if (!challenge || challenge.email !== email || challenge.expiresAt < Date.now()) {
      throw new AuthFailure('code')
    }
    const result = await this.session(() => challenge.verify({ token: code }), 'code')
    this.challenge = null
    return result
  }

  async logout(): Promise<void> {
    this.challenge = null
    // Revoke this session only, not all devices or the website's session.
    await this.auth.signOut()
  }
}
