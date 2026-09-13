import type { AccountProfile } from '@mycopilot/host-api'
import { ACCOUNT_CONFIG } from './accountConfig'
import { AuthFailure, type CloudSession } from './CloudBaseAuthDriver'

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

export function parseAccountProfile(body: unknown, email: string): AccountProfile {
  const data = record(record(body).data)
  const profile = record(data.profile)
  if (
    !data.profile ||
    typeof data.userId !== 'string' ||
    !data.userId ||
    profile.userId !== data.userId
  ) {
    throw new AuthFailure('profile')
  }
  if (profile.status !== 'active') throw new AuthFailure('inactive')
  const text = (value: unknown, limit = 200): string =>
    typeof value === 'string' ? value.slice(0, limit) : ''
  const avatar = profile.avatarDataUrl
  // This API returns inline avatars. Do not interpret arbitrary URLs, SVG, or HTML as images.
  const avatarDataUrl =
    typeof avatar === 'string' &&
    avatar.length <= 20_000 &&
    /^data:image\/(?:png|jpeg|webp);base64,[a-zA-Z0-9+/=\r\n]+$/.test(avatar)
      ? avatar
      : null
  return {
    userId: data.userId,
    displayName: text(profile.displayName),
    email,
    avatarDataUrl,
    occupation: text(profile.occupation),
    organization: text(profile.organization)
  }
}

export async function fetchAccountProfile(session: CloudSession): Promise<AccountProfile> {
  let response: Response
  try {
    response = await fetch(`${ACCOUNT_CONFIG.api}/v1/me`, {
      headers: { Authorization: `Bearer ${session.access_token}`, Accept: 'application/json' },
      signal: AbortSignal.timeout(15_000),
      redirect: 'error',
      cache: 'no-store'
    })
  } catch {
    throw new AuthFailure('network')
  }
  if (response.status === 401) throw new AuthFailure('expired')
  if (response.status === 403) throw new AuthFailure('inactive')
  if (response.status === 429) throw new AuthFailure('rateLimit')
  if (response.status >= 500) throw new AuthFailure('network')
  if (response.status !== 200) throw new AuthFailure('profile')
  try {
    return parseAccountProfile(await response.json(), session.email)
  } catch (error) {
    if (error instanceof AuthFailure) throw error
    throw new AuthFailure('profile')
  }
}
