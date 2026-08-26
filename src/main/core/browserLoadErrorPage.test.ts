import { describe, expect, it } from 'vitest'
import {
  classifyBrowserLoadError,
  createBrowserSurfaceCrashError,
  createBrowserLoadErrorPageHtml,
  createBrowserSurfaceLoadError
} from '../browser/BrowserLoadErrorPage'

describe('Browser load error classification', () => {
  it.each([
    ['ERR_INTERNET_DISCONNECTED', 'offline'],
    ['net::ERR_NAME_NOT_RESOLVED', 'dns'],
    ['DNS_PROBE_POSSIBLE', 'dns'],
    ['ERR_CONNECTION_REFUSED', 'connection_refused'],
    ['ERR_CONNECTION_TIMED_OUT', 'timeout'],
    ['ERR_TIMED_OUT', 'timeout'],
    ['ERR_CERT_DATE_INVALID', 'certificate'],
    ['ERR_FAILED', 'generic']
  ] as const)('maps %s to %s', (description, expected) => {
    expect(classifyBrowserLoadError(description)).toBe(expected)
  })
})

describe('Browser internal load error page', () => {
  it('keeps the internal URL Main-only while preserving structured navigation identity', () => {
    const error = createBrowserSurfaceLoadError({
      errorCode: -106,
      errorDescription: 'net::ERR_INTERNET_DISCONNECTED',
      failedUrl: 'https://example.com/private?q=secret',
      generation: 7,
      locale: 'zh-CN',
      navigationEpoch: 12,
      nonce: 'fixed-test-nonce-0001',
      actionToken: 'fixed-test-action-token-00000001'
    })

    expect(error).toMatchObject({
      kind: 'offline',
      errorCode: -106,
      errorDescription: 'ERR_INTERNET_DISCONNECTED',
      failedUrl: 'https://example.com/private?q=secret',
      generation: 7,
      navigationEpoch: 12,
      title: '无法访问此站点'
    })
    const html = error.internalPageHtml
    expect(html).toMatch(/^<!doctype html>/u)
    expect(html).not.toContain('private?q=secret')
    expect(html).not.toContain('q=secret')
    expect(html).toContain(
      'mycopilot-browser-internal://action/retry/fixed-test-action-token-00000001'
    )
  })

  it('escapes HTML, attributes, and inline JSON without loading remote resources', () => {
    const html = createBrowserLoadErrorPageHtml(
      {
        kind: 'generic',
        errorCode: -2,
        errorDescription: 'ERR_FAILED</script>',
        failedUrl:
          'https://example.com/?next=%3C%2Fscript%3E%3Cimg%20src=x%20onerror=alert(1)%3E&safe=1',
        title: '<img src=x onerror=alert(1)>',
        heading: '</style><script>alert(1)</script>',
        summary: '" onmouseover="alert(1)',
        suggestions: ['<script>alert(1)</script>']
      },
      {
        actionUrl: 'mycopilot-browser-internal://action/retry/fixed-test-action-token-00000001',
        nonce: 'fixed-test-nonce-0001'
      }
    )
    expect(html).toContain("default-src 'none'")
    expect(html).toContain("style-src 'nonce-fixed-test-nonce-0001'")
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
    expect(html).toContain('&lt;/style&gt;&lt;script&gt;alert(1)&lt;/script&gt;')
    expect(html).not.toContain('%3C%2Fscript%3E')
    expect(html).not.toContain('safe=1')
    expect(html).toContain(
      'mycopilot-browser-internal://action/retry/fixed-test-action-token-00000001'
    )
    expect(html).not.toContain('<img src=x onerror=alert(1)>')
    expect(html).not.toMatch(/https?:\/\/[^"']+\.(?:js|css|png|svg)/u)
  })

  it('builds a distinct localized renderer recovery document without exposing the logical URL', () => {
    const error = createBrowserSurfaceCrashError({
      actionToken: 'fixed-crash-action-token-0000001',
      generation: 4,
      kind: 'renderer_crashed',
      locale: 'en-US',
      logicalUrl: 'https://example.com/private?session=secret',
      navigationEpoch: 9,
      nonce: 'fixed-test-nonce-0001'
    })
    const html = error.internalPageHtml

    expect(error.kind).toBe('renderer_crashed')
    expect(html).toContain('The page crashed')
    expect(html).toContain(
      'mycopilot-browser-internal://action/recover/fixed-crash-action-token-0000001'
    )
    expect(html).not.toContain('session=secret')
  })
})
