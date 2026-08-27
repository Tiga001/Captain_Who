import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const electronHarness = vi.hoisted(() => ({
  handle: vi.fn(),
  registerSchemesAsPrivileged: vi.fn()
}))

vi.mock('electron', () => ({
  app: { getPath: () => tmpdir() },
  protocol: {
    handle: electronHarness.handle,
    registerSchemesAsPrivileged: electronHarness.registerSchemesAsPrivileged
  }
}))

import { FaviconResourceCache, type FaviconNetworkSession } from '../resources/FaviconResourceCache'

type NetworkFetchInput = Parameters<FaviconNetworkSession['fetch']>[0]
type NetworkFetchInit = Parameters<FaviconNetworkSession['fetch']>[1]

const cacheDirectories: string[] = []

afterEach(async () => {
  electronHarness.handle.mockReset()
  electronHarness.registerSchemesAsPrivileged.mockReset()
  await Promise.all(
    cacheDirectories.splice(0).map((directory) => rm(directory, { force: true, recursive: true }))
  )
})

async function createCache(networkSession: FaviconNetworkSession) {
  const cacheDirectory = await mkdtemp(join(tmpdir(), 'mycopilot-favicon-cache-'))
  cacheDirectories.push(cacheDirectory)
  return {
    cache: new FaviconResourceCache({ cacheDirectory, networkSession }),
    cacheDirectory
  }
}

function createNetworkSession(options: {
  addresses?: readonly string[]
  fetch?: (url: string, init: NetworkFetchInit) => Promise<Response>
  proxy?: string
}) {
  const addresses = options.addresses ?? ['93.184.216.34']
  const fetch = vi.fn(async (input: NetworkFetchInput, init?: NetworkFetchInit) => {
    const url = typeof input === 'string' ? input : input.url
    return options.fetch?.(url, init) ?? new Response(null, { status: 404 })
  })
  const resolveHost = vi.fn(async () => ({
    endpoints: addresses.map((address) => ({
      address,
      family: address.includes(':') ? ('ipv6' as const) : ('ipv4' as const)
    }))
  }))
  const resolveProxy = vi.fn(async () => options.proxy ?? 'DIRECT')
  return {
    fetch,
    networkSession: { fetch, resolveHost, resolveProxy } as FaviconNetworkSession,
    resolveHost,
    resolveProxy
  }
}

function imageResponse(bytes = [0x89, 0x50, 0x4e, 0x47]): Response {
  return new Response(new Uint8Array(bytes), {
    headers: { 'content-type': 'image/png' },
    status: 200
  })
}

describe('FaviconResourceCache', () => {
  it('uses an exclusive proxy for standard Fake-IP ranges and repopulates after clear', async () => {
    const pageUrl = 'https://www.public-site.com/'
    const iconUrl = 'https://www.public-site.com/assets/favicon.png'
    const harness = createNetworkSession({
      addresses: ['198.18.0.42', 'fdfe:dcba:9876::2a'],
      proxy: 'PROXY 127.0.0.1:7897',
      fetch: async (url) => {
        if (url === pageUrl) {
          return new Response('<link rel="icon" href="/assets/favicon.png">', {
            headers: { 'content-type': 'text/html' },
            status: 200
          })
        }
        return url === iconUrl ? imageResponse() : new Response(null, { status: 404 })
      }
    })
    const { cache, cacheDirectory } = await createCache(harness.networkSession)

    const first = await cache.resolveFavicon({ pageUrl })

    expect(first.url).toMatch(/^mycopilot-resource:\/\/favicon\/[a-f0-9]{64}$/u)
    expect(await readdir(cacheDirectory)).toHaveLength(1)
    expect(harness.fetch).toHaveBeenCalledTimes(2)
    expect(harness.resolveProxy).toHaveBeenCalled()
    expect(harness.fetch.mock.calls[0]?.[1]).toMatchObject({
      cache: 'no-store',
      credentials: 'omit',
      redirect: 'manual',
      referrerPolicy: 'no-referrer'
    })

    await cache.clear()
    const second = await cache.resolveFavicon({ pageUrl })

    expect(second).toEqual(first)
    expect(await readdir(cacheDirectory)).toHaveLength(1)
    expect(harness.fetch).toHaveBeenCalledTimes(4)
  })

  it('keeps candidate failures isolated and falls back to the origin favicon', async () => {
    const pageUrl = 'https://www.public-site.com/docs'
    const suppliedIconUrl = 'https://cdn.public-site.com/oversized.png'
    const fallbackUrl = 'https://www.public-site.com/favicon.ico'
    const requestedUrls: string[] = []
    const harness = createNetworkSession({
      fetch: async (url) => {
        requestedUrls.push(url)
        if (url === suppliedIconUrl) {
          return new Response(new Uint8Array([1]), {
            headers: {
              'content-length': `${64 * 1024 + 1}`,
              'content-type': 'image/png'
            },
            status: 200
          })
        }
        if (url === pageUrl) throw new Error('HTML discovery unavailable')
        return url === fallbackUrl ? imageResponse() : new Response(null, { status: 404 })
      }
    })
    const { cache } = await createCache(harness.networkSession)

    const result = await cache.resolveFavicon({ faviconUrl: suppliedIconUrl, pageUrl })

    expect(result.url).toMatch(/^mycopilot-resource:\/\/favicon\//u)
    expect(requestedUrls).toEqual([suppliedIconUrl, pageUrl, fallbackUrl])
  })

  it('rejects Fake-IP routes without a strict proxy and never widens ordinary private DNS', async () => {
    const cases = [
      {
        addresses: ['198.18.0.42', 'fdfe:dcba:9876::2a'],
        pageUrl: 'https://www.public-site.com/',
        proxy: 'DIRECT'
      },
      {
        addresses: ['198.18.0.42'],
        pageUrl: 'https://www.public-site.com/',
        proxy: 'PROXY 127.0.0.1:7897; DIRECT'
      },
      {
        addresses: ['192.168.1.20'],
        pageUrl: 'https://www.public-site.com/',
        proxy: 'PROXY 127.0.0.1:7897'
      },
      {
        addresses: ['198.18.0.42'],
        pageUrl: 'https://service.internal/',
        proxy: 'PROXY 127.0.0.1:7897'
      }
    ] as const

    for (const testCase of cases) {
      const harness = createNetworkSession(testCase)
      const { cache } = await createCache(harness.networkSession)

      await expect(cache.resolveFavicon({ pageUrl: testCase.pageUrl })).resolves.toEqual({
        url: null
      })
      expect(harness.fetch).not.toHaveBeenCalled()
    }

    const literalHarness = createNetworkSession({
      addresses: ['93.184.216.34'],
      proxy: 'PROXY 127.0.0.1:7897'
    })
    const { cache: literalCache } = await createCache(literalHarness.networkSession)
    await expect(literalCache.resolveFavicon({ pageUrl: 'https://198.18.0.42/' })).resolves.toEqual(
      { url: null }
    )
    expect(literalHarness.resolveHost).not.toHaveBeenCalled()
    expect(literalHarness.fetch).not.toHaveBeenCalled()
  })

  it('does not let an in-flight request restore files after a clear', async () => {
    let releaseFirstFetch: ((response: Response) => void) | undefined
    const firstFetch = new Promise<Response>((resolve) => {
      releaseFirstFetch = resolve
    })
    const harness = createNetworkSession({
      fetch: async () => {
        if (harness.fetch.mock.calls.length === 1) return firstFetch
        return imageResponse()
      }
    })
    const { cache, cacheDirectory } = await createCache(harness.networkSession)
    const input = {
      faviconUrl: 'https://www.public-site.com/favicon.png',
      pageUrl: 'https://www.public-site.com/'
    }

    const pending = cache.resolveFavicon(input)
    await vi.waitFor(() => expect(harness.fetch).toHaveBeenCalledTimes(1))
    await cache.clear()
    releaseFirstFetch?.(imageResponse())

    await expect(pending).resolves.toEqual({ url: null })
    await expect(readdir(cacheDirectory)).rejects.toMatchObject({ code: 'ENOENT' })

    await expect(cache.resolveFavicon(input)).resolves.toEqual({
      url: expect.stringMatching(/^mycopilot-resource:\/\/favicon\//u)
    })
    expect(await readdir(cacheDirectory)).toHaveLength(1)
  })

  it('does not follow favicon redirects into a literal private address', async () => {
    const requestedUrls: string[] = []
    const suppliedIconUrl = 'https://cdn.public-site.com/favicon.png'
    const harness = createNetworkSession({
      fetch: async (url) => {
        requestedUrls.push(url)
        if (url === suppliedIconUrl) {
          return new Response(null, {
            headers: { location: 'http://127.0.0.1:8080/private.png' },
            status: 302
          })
        }
        return new Response(null, { status: 404 })
      }
    })
    const { cache } = await createCache(harness.networkSession)

    await expect(
      cache.resolveFavicon({
        faviconUrl: suppliedIconUrl,
        pageUrl: 'https://www.public-site.com/'
      })
    ).resolves.toEqual({ url: null })
    expect(requestedUrls).not.toContain('http://127.0.0.1:8080/private.png')
  })
})
