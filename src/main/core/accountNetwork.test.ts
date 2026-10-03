import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const electron = vi.hoisted(() => ({
  fetch: vi.fn(),
  setProxy: vi.fn(),
  fromPartition: vi.fn()
}))
vi.mock('electron', () => ({ session: { fromPartition: electron.fromPartition } }))

import { fetchAccountProfile } from '../auth/AccountApiClient'
import { fetchAccountLicense } from '../auth/LicenseApiClient'
import { CloudBaseAuthDriver } from '../auth/CloudBaseAuthDriver'
import { createAccountFetch } from '../auth/AccountNetwork'
import { createCloudBaseNetworkRequest } from '../auth/CloudBaseNetworkRequest'

const nodeFetch = vi.fn(() => {
  throw new Error('Node fetch must not carry account requests')
})
beforeEach(() => {
  vi.clearAllMocks()
  electron.fromPartition.mockReturnValue(electron)
  electron.setProxy.mockResolvedValue(undefined)
  vi.stubGlobal('fetch', nodeFetch)
  vi.spyOn(console, 'error').mockImplementation(() => undefined)
})
afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('account network transport', () => {
  it('loads account profiles through an isolated Electron session instead of Node fetch', async () => {
    electron.fetch.mockResolvedValueOnce(
      Response.json({ data: { userId: 'a', profile: { userId: 'a', status: 'active' } } })
    )
    await expect(
      fetchAccountProfile({
        access_token: 'synthetic-access',
        refresh_token: 'synthetic-refresh',
        email: 'identity@example.com'
      })
    ).resolves.toMatchObject({ userId: 'a' })
    expect(nodeFetch).not.toHaveBeenCalled()
    expect(electron.fromPartition).toHaveBeenCalledWith('captain-who-account', { cache: false })
    expect(electron.setProxy).toHaveBeenCalledWith({ mode: 'system' })
  })

  it('loads licenses through Electron while retaining authenticated GET semantics', async () => {
    const license = {
      allowed: true,
      reason: 'active',
      expiresAt: null,
      verifiedAt: '2026-09-13T06:00:00.000Z',
      cacheValidUntil: '2026-09-14T06:00:00.000Z'
    }
    electron.fetch.mockResolvedValueOnce(Response.json({ data: license }))
    await expect(
      fetchAccountLicense('synthetic-access', new AbortController().signal)
    ).resolves.toEqual(license)
    expect(nodeFetch).not.toHaveBeenCalled()
    expect(electron.fetch).toHaveBeenCalledWith(
      expect.stringContaining('/v1/me/license'),
      expect.objectContaining({
        headers: { Authorization: 'Bearer synthetic-access', Accept: 'application/json' },
        redirect: 'error',
        credentials: 'omit',
        cache: 'no-store'
      })
    )
  })

  it.each(['login', 'sendCode', 'restore', 'refresh'] as const)(
    'routes the real CloudBase SDK %s flow through the same Electron transport',
    async (operation) => {
      electron.fetch.mockImplementation(async () =>
        Response.json({
          error: 'resource_exhausted',
          error_code: '8',
          error_description: 'res_stopped'
        })
      )
      const driver = new CloudBaseAuthDriver()
      const pending =
        operation === 'login'
          ? driver.login('identity@example.com', 'synthetic-password')
          : operation === 'sendCode'
            ? driver.sendCode('identity@example.com')
            : driver[operation]({
                access_token: 'synthetic-access',
                refresh_token: 'synthetic-refresh'
              })
      await expect(pending).rejects.toMatchObject({ code: 'serviceUnavailable' })
      expect(nodeFetch).not.toHaveBeenCalled()
      expect(electron.fetch).toHaveBeenCalled()
    }
  )

  it('waits for session configuration before dispatching and uses the latest session route', async () => {
    let ready!: (session: typeof electron) => void
    const initializing = new Promise<typeof electron>((resolve) => {
      ready = resolve
    })
    const transport = createAccountFetch(() => initializing)
    let route = 'proxy'
    const paths: string[] = []
    electron.fetch.mockImplementation(async () => {
      paths.push(route)
      return Response.json({ ok: true })
    })
    const first = transport('https://account.example.com/login')
    expect(electron.fetch).not.toHaveBeenCalled()
    ready(electron)
    await first
    route = 'direct'
    await transport('https://account.example.com/login')
    expect(paths).toEqual(['proxy', 'direct'])
    expect(nodeFetch).not.toHaveBeenCalled()
  })

  it('enforces cookie, cache and redirect isolation while preserving explicit SDK authorization', async () => {
    electron.fetch.mockResolvedValueOnce(Response.json({ ok: true }))
    const transport = createAccountFetch(async () => electron)
    await transport('https://account.example.com/login', {
      headers: { Authorization: 'Bearer synthetic-access' },
      redirect: 'follow',
      credentials: 'include',
      cache: 'force-cache'
    })
    expect(electron.fetch).toHaveBeenCalledWith('https://account.example.com/login', {
      headers: { Authorization: 'Bearer synthetic-access' },
      redirect: 'error',
      credentials: 'omit',
      cache: 'no-store',
      signal: expect.any(AbortSignal)
    })
  })

  it.each(['http://account.example.com', 'https://username:password@account.example.com'])(
    'rejects unsafe account URL %s before dispatch',
    async (url) => {
      const getSession = vi.fn(async () => electron)
      await expect(createAccountFetch(getSession)(url)).rejects.toThrow(
        'Invalid account request URL'
      )
      expect(getSession).not.toHaveBeenCalled()
    }
  )

  it('cancels while proxy initialization is pending without dispatching late requests', async () => {
    const controller = new AbortController()
    let ready!: (session: typeof electron) => void
    const transport = createAccountFetch(
      () =>
        new Promise((resolve) => {
          ready = resolve
        })
    )
    const pending = transport('https://account.example.com/login', { signal: controller.signal })
    controller.abort()
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
    ready(electron)
    await Promise.resolve()
    expect(electron.fetch).not.toHaveBeenCalled()
  })

  it('bounds proxy initialization with the same 15 second timeout as network transfer', async () => {
    const deadline = new AbortController()
    const timeout = vi.spyOn(AbortSignal, 'timeout').mockReturnValue(deadline.signal)
    const transport = createAccountFetch(() => new Promise(() => undefined))
    const pending = transport('https://account.example.com/login')
    deadline.abort(new DOMException('Request deadline', 'TimeoutError'))
    await expect(pending).rejects.toMatchObject({ name: 'TimeoutError' })
    expect(timeout).toHaveBeenCalledWith(15_000)
    expect(electron.fetch).not.toHaveBeenCalled()
  })

  it('does not retry or fall back to Node when the selected Electron route fails', async () => {
    electron.fetch.mockRejectedValueOnce(new Error('ERR_PROXY_CONNECTION_FAILED'))
    const transport = createAccountFetch(async () => electron)
    await expect(transport('https://account.example.com/login')).rejects.toThrow(
      'ERR_PROXY_CONNECTION_FAILED'
    )
    expect(electron.fetch).toHaveBeenCalledTimes(1)
    expect(nodeFetch).not.toHaveBeenCalled()
  })
})

describe('CloudBase transport adapter', () => {
  it('serializes JSON bodies without changing the SDK request or losing authorization', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(Response.json({ user: { id: 'synthetic' } }))
    const body = { email: 'identity@example.com', token: 'synthetic-code' }
    const options = { method: 'POST', body, headers: { Authorization: 'Bearer synthetic-access' } }
    await expect(
      createCloudBaseNetworkRequest(fetch)('https://account.example.com/login', options)
    ).resolves.toEqual({ user: { id: 'synthetic' } })
    expect(options.body).toBe(body)
    expect(fetch).toHaveBeenCalledWith('https://account.example.com/login', {
      method: 'POST',
      body: JSON.stringify(body),
      headers: options.headers,
      signal: undefined
    })
  })

  it('preserves SDK error codes so expired credentials and captcha requirements still work', async () => {
    const payload = { error: 'invalid_grant', error_code: '401', request_id: 'synthetic' }
    const fetch = vi.fn().mockResolvedValueOnce(Response.json(payload, { status: 401 }))
    await expect(
      createCloudBaseNetworkRequest(fetch)('https://account.example.com/login')
    ).rejects.toEqual(payload)
  })

  it.each(['network', 'invalid-json', 'http'] as const)(
    'sanitizes %s failures for the SDK',
    async (failure) => {
      const fetch =
        failure === 'network'
          ? vi.fn().mockRejectedValueOnce(new Error('secret URL or credential'))
          : vi
              .fn()
              .mockResolvedValueOnce(
                failure === 'invalid-json'
                  ? new Response('invalid secret response')
                  : Response.json({ message: 'private upstream diagnostics' }, { status: 502 })
              )
      await expect(
        createCloudBaseNetworkRequest(fetch)('https://account.example.com/login')
      ).rejects.toEqual({
        error: 'unreachable',
        error_description: 'Account network request failed'
      })
    }
  )
})
