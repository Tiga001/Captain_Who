import { randomBytes } from 'node:crypto'
import type { Session } from 'electron'

export const BROWSER_INTERNAL_PAGE_SCHEME = 'mycopilot-browser-internal'

const INTERNAL_PAGE_URL_PREFIX = `${BROWSER_INTERNAL_PAGE_SCHEME}://page/`
const INTERNAL_PAGE_TOKEN_PATTERN = /^[A-Za-z0-9_-]{32}$/u
const MAX_INTERNAL_PAGE_DOCUMENTS = 2_048
const MAX_INTERNAL_PAGE_HTML_LENGTH = 1_048_576
const STRICT_NONE_CSP_DIRECTIVES = [
  'default-src',
  'base-uri',
  'connect-src',
  'form-action',
  'frame-ancestors',
  'frame-src',
  'object-src',
  'img-src',
  'media-src'
] as const
const STRICT_NONCE_CSP_DIRECTIVES = ['style-src', 'script-src'] as const

interface StoredInternalPage {
  csp: string
  html: string
}

export interface BrowserInternalPageStoreLike {
  register(html: string): { url: string }
  release(url: string): void
  shutdown(): Promise<void>
}

/**
 * Session-scoped, Main-owned carrier for browser recovery documents.
 *
 * The opaque URL is only a capability lookup key. Logical page URLs and recovery HTML never enter
 * the URL itself, while Chromium still treats the document as an ordinary navigation-history item.
 */
export class BrowserInternalPageStore implements BrowserInternalPageStoreLike {
  private readonly documents = new Map<string, StoredInternalPage>()
  private disposed = false
  private installed = false

  constructor(private readonly targetSession: Session) {}

  install(): void {
    if (this.disposed) throw new Error('browser.internal_page_store.closed')
    if (this.installed) return
    this.targetSession.protocol.handle(BROWSER_INTERNAL_PAGE_SCHEME, (request) =>
      this.handleRequest(request)
    )
    this.installed = true
  }

  register(html: string): { url: string } {
    if (this.disposed || !this.installed) {
      throw new Error('browser.internal_page_store.unavailable')
    }
    if (
      html.length === 0 ||
      html.length > MAX_INTERNAL_PAGE_HTML_LENGTH ||
      html.includes('\0') ||
      this.documents.size >= MAX_INTERNAL_PAGE_DOCUMENTS
    ) {
      throw new Error('browser.internal_page_store.invalid_document')
    }
    const csp = extractStrictContentSecurityPolicy(html)
    let url: string
    do {
      url = `${INTERNAL_PAGE_URL_PREFIX}${randomBytes(24).toString('base64url')}`
    } while (this.documents.has(url))
    this.documents.set(url, { csp, html })
    return { url }
  }

  release(url: string): void {
    if (!isBrowserInternalPageUrl(url)) return
    this.documents.delete(url)
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    this.documents.clear()
    if (!this.installed) return
    this.installed = false
    this.targetSession.protocol.unhandle(BROWSER_INTERNAL_PAGE_SCHEME)
  }

  private handleRequest(request: Request): Response {
    if (request.method !== 'GET') return emptyResponse(405)
    const document = isBrowserInternalPageUrl(request.url)
      ? this.documents.get(request.url)
      : undefined
    if (!document) return emptyResponse(404)
    return new Response(document.html, {
      headers: {
        'cache-control': 'no-store, max-age=0',
        'content-security-policy': document.csp,
        'content-type': 'text/html; charset=utf-8',
        'x-content-type-options': 'nosniff'
      },
      status: 200
    })
  }
}

export function isBrowserInternalPageUrl(value: string): boolean {
  if (!value.startsWith(INTERNAL_PAGE_URL_PREFIX) || value.length > 256) return false
  try {
    const parsed = new URL(value)
    return (
      parsed.protocol === `${BROWSER_INTERNAL_PAGE_SCHEME}:` &&
      parsed.hostname === 'page' &&
      parsed.port === '' &&
      parsed.username === '' &&
      parsed.password === '' &&
      parsed.search === '' &&
      parsed.hash === '' &&
      INTERNAL_PAGE_TOKEN_PATTERN.test(parsed.pathname.slice(1)) &&
      parsed.toString() === value
    )
  } catch {
    return false
  }
}

function extractStrictContentSecurityPolicy(html: string): string {
  const matches = [
    ...html.matchAll(/<meta\s+http-equiv="Content-Security-Policy"\s+content="([^"]+)">/giu)
  ]
  const csp = matches.length === 1 ? matches[0]?.[1] : undefined
  if (!csp || csp.length > 2_048) throw new Error('browser.internal_page_store.unsafe_csp')

  const directives = new Map<string, readonly string[]>()
  for (const rawDirective of csp.split(';')) {
    const parts = rawDirective.trim().split(/\s+/u).filter(Boolean)
    if (parts.length === 0) continue
    const [name, ...sources] = parts
    if (!name || directives.has(name)) throw new Error('browser.internal_page_store.unsafe_csp')
    directives.set(name, sources)
  }
  const expectedDirectiveCount =
    STRICT_NONE_CSP_DIRECTIVES.length + STRICT_NONCE_CSP_DIRECTIVES.length
  if (directives.size !== expectedDirectiveCount) {
    throw new Error('browser.internal_page_store.unsafe_csp')
  }
  for (const name of STRICT_NONE_CSP_DIRECTIVES) {
    const sources = directives.get(name)
    if (sources?.length !== 1 || sources[0] !== "'none'") {
      throw new Error('browser.internal_page_store.unsafe_csp')
    }
  }
  for (const name of STRICT_NONCE_CSP_DIRECTIVES) {
    const sources = directives.get(name)
    if (
      sources?.length !== 1 ||
      (sources[0] !== "'none'" && !/^'nonce-[A-Za-z0-9_-]{16,128}'$/u.test(sources[0] ?? ''))
    ) {
      throw new Error('browser.internal_page_store.unsafe_csp')
    }
  }
  return csp
}

function emptyResponse(status: number): Response {
  return new Response(null, {
    headers: {
      'cache-control': 'no-store, max-age=0',
      'content-security-policy': "default-src 'none'; frame-ancestors 'none'",
      'x-content-type-options': 'nosniff'
    },
    status
  })
}
