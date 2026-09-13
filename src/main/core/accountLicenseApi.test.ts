import { afterEach, describe, expect, it, vi } from 'vitest'
import { fetchAccountLicense } from '../auth/LicenseApiClient'
import { fetchAccountProfile } from '../auth/AccountApiClient'
import { ACCOUNT_CONFIG } from '../auth/accountConfig'

afterEach(() => vi.unstubAllGlobals())
const result = {
  allowed: true,
  reason: 'active',
  expiresAt: null,
  verifiedAt: '2026-09-13T06:00:00.000Z',
  cacheValidUntil: '2026-09-14T06:00:00.000Z'
}
describe('license-only account HTTP boundary', () => {
  it('sends only an authenticated GET, never device or usage data', async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: result })))
    vi.stubGlobal('fetch', fetch)
    expect(await fetchAccountLicense('synthetic-access', new AbortController().signal)).toEqual(
      result
    )
    expect(fetch).toHaveBeenCalledWith(`${ACCOUNT_CONFIG.api}/v1/me/license`, {
      headers: { Authorization: 'Bearer synthetic-access', Accept: 'application/json' },
      signal: expect.any(AbortSignal),
      redirect: 'error',
      cache: 'no-store'
    })
    expect(fetch.mock.calls[0][1]).not.toHaveProperty('body')
  })
  it.each([
    [401, {}, 'network', true],
    [403, {}, 'network', true],
    [404, {}, 'network', false],
    [503, { error: { code: 'LICENSE_NOT_PROVISIONED' } }, 'notProvisioned', false],
    [429, { error: { code: 'RATE_LIMITED' } }, 'network', false],
    [200, { data: { allowed: true } }, 'invalidResponse', false]
  ])('does not invent permission for HTTP %s', async (status, body, code, unauthorized) => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response(JSON.stringify(body), { status: status as number }))
    )
    await expect(
      fetchAccountLicense('synthetic-access', new AbortController().signal)
    ).rejects.toMatchObject({ code, unauthorized })
  })
  it('requests profile without entitlements during ordinary account validation', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(
          JSON.stringify({ data: { userId: 'a', profile: { userId: 'a', status: 'active' } } })
        )
      )
    vi.stubGlobal('fetch', fetch)
    await fetchAccountProfile({
      access_token: 'synthetic-access',
      refresh_token: 'synthetic-refresh',
      email: 'test@example.com'
    })
    expect(fetch.mock.calls[0][0]).toBe(`${ACCOUNT_CONFIG.api}/v1/me?includeEntitlements=false`)
    expect(fetch.mock.calls[0][1]).not.toHaveProperty('body')
  })
})
