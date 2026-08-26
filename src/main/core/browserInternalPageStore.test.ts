import type { Session } from 'electron'
import { describe, expect, it, vi } from 'vitest'

import {
  BROWSER_INTERNAL_PAGE_SCHEME,
  BrowserInternalPageStore,
  isBrowserInternalPageUrl
} from '../browser/BrowserInternalPageStore'

const SAFE_HTML = `<!doctype html>
<html><head>
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; base-uri 'none'; connect-src 'none'; form-action 'none'; frame-ancestors 'none'; frame-src 'none'; object-src 'none'; img-src 'none'; media-src 'none'; style-src 'none'; script-src 'none'">
  <title>Internal recovery</title>
</head><body>Safe recovery document</body></html>`

function createHarness() {
  let handler: ((request: Request) => Response | Promise<Response>) | undefined
  const handle = vi.fn(
    (_scheme: string, nextHandler: (request: Request) => Response | Promise<Response>) => {
      handler = nextHandler
    }
  )
  const unhandle = vi.fn()
  const targetSession = {
    protocol: { handle, unhandle }
  } as unknown as Session
  const store = new BrowserInternalPageStore(targetSession)
  return {
    handle,
    request: async (url: string, method = 'GET') => {
      if (!handler) throw new Error('fixture protocol handler is not installed')
      return await handler(new Request(url, { method }))
    },
    store,
    unhandle
  }
}

describe('BrowserInternalPageStore', () => {
  it('serves only exact registered opaque URLs with response-level security headers', async () => {
    const harness = createHarness()
    harness.store.install()
    const { url } = harness.store.register(SAFE_HTML)

    expect(harness.handle).toHaveBeenCalledWith(BROWSER_INTERNAL_PAGE_SCHEME, expect.any(Function))
    expect(isBrowserInternalPageUrl(url)).toBe(true)
    expect(url).not.toContain('Internal')

    const response = await harness.request(url)
    expect(response.status).toBe(200)
    expect(response.headers.get('cache-control')).toContain('no-store')
    expect(response.headers.get('content-security-policy')).toContain("default-src 'none'")
    expect(response.headers.get('x-content-type-options')).toBe('nosniff')
    await expect(response.text()).resolves.toBe(SAFE_HTML)

    await expect(harness.request(`${url}?forged=1`)).resolves.toMatchObject({ status: 404 })
    await expect(harness.request(url, 'POST')).resolves.toMatchObject({ status: 405 })

    harness.store.release(url)
    await expect(harness.request(url)).resolves.toMatchObject({ status: 404 })
    await harness.store.shutdown()
    expect(harness.unhandle).toHaveBeenCalledWith(BROWSER_INTERNAL_PAGE_SCHEME)
  })

  it('fails closed for missing or weakened CSP and after shutdown', async () => {
    const harness = createHarness()
    harness.store.install()

    expect(() => harness.store.register('<html><body>unsafe</body></html>')).toThrow(
      'browser.internal_page_store.unsafe_csp'
    )
    expect(() =>
      harness.store.register(
        SAFE_HTML.replace("connect-src 'none'", 'connect-src https://example.test')
      )
    ).toThrow('browser.internal_page_store.unsafe_csp')
    expect(() =>
      harness.store.register(
        SAFE_HTML.replace(
          "connect-src 'none'",
          "connect-src https://example.test; connect-src 'none'"
        )
      )
    ).toThrow('browser.internal_page_store.unsafe_csp')
    expect(() => harness.store.register(SAFE_HTML.replace("media-src 'none'; ", ''))).toThrow(
      'browser.internal_page_store.unsafe_csp'
    )

    await harness.store.shutdown()
    expect(() => harness.store.register(SAFE_HTML)).toThrow(
      'browser.internal_page_store.unavailable'
    )
  })

  it('rejects malformed, decorated, and non-canonical internal URLs', () => {
    const valid = `${BROWSER_INTERNAL_PAGE_SCHEME}://page/${'A'.repeat(32)}`
    expect(isBrowserInternalPageUrl(valid)).toBe(true)
    expect(isBrowserInternalPageUrl(`${valid}#fragment`)).toBe(false)
    expect(isBrowserInternalPageUrl(`${valid}?query=1`)).toBe(false)
    expect(
      isBrowserInternalPageUrl(`${BROWSER_INTERNAL_PAGE_SCHEME}://other/${'A'.repeat(32)}`)
    ).toBe(false)
    expect(isBrowserInternalPageUrl(`${BROWSER_INTERNAL_PAGE_SCHEME}://page/short`)).toBe(false)
    expect(isBrowserInternalPageUrl('data:text/html,<h1>forged</h1>')).toBe(false)
  })
})
