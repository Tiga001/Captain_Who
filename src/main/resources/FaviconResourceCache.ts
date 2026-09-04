import { app, protocol, type Session } from 'electron'
import { createHash } from 'crypto'
import { BlockList, isIP } from 'net'
import { extname, join } from 'path'
import { mkdir, readFile, readdir, rm, stat, writeFile } from 'fs/promises'
import type { ResourceFaviconRequest, ResourceFaviconResponse } from '@mycopilot/protocol'
import { classifyAddress } from '../browser/BrowserNetworkPolicy'

const RESOURCE_SCHEME = 'mycopilot-resource'
const FAVICON_HOST = 'favicon'
const FAVICON_CACHE_DIRECTORY = 'favicons'
const FAVICON_FETCH_TIMEOUT_MS = 2_500
const FAVICON_MAX_BYTES = 64 * 1024
const FAVICON_HTML_MAX_BYTES = 512 * 1024
const FAVICON_MAX_REDIRECTS = 3
const FAVICON_CACHE_MAX_FILES = 256
const FAVICON_CACHE_MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000
const FAVICON_MAX_CONCURRENT_RESOLUTIONS = 6
const FAVICON_NEGATIVE_CACHE_TTL_MS = 5 * 60 * 1000
const FAVICON_NEGATIVE_CACHE_MAX_ENTRIES = 512
const FAVICON_USER_AGENT = 'CaptainWho/1.0 favicon resolver'
const PROXY_FAKE_IP_RANGES = new BlockList()

PROXY_FAKE_IP_RANGES.addSubnet('198.18.0.0', 15, 'ipv4')
PROXY_FAKE_IP_RANGES.addSubnet('fdfe:dcba:9876::', 64, 'ipv6')

const FAVICON_EXTENSIONS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'ico', 'svg'] as const

const MIME_BY_EXTENSION: Record<string, string> = {
  '.gif': 'image/gif',
  '.ico': 'image/x-icon',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp'
}

const EXTENSION_BY_MIME: Record<string, string> = {
  'image/gif': 'gif',
  'image/jpeg': 'jpg',
  'image/png': 'png',
  'image/svg+xml': 'svg',
  'image/vnd.microsoft.icon': 'ico',
  'image/webp': 'webp',
  'image/x-icon': 'ico'
}

interface CachedFaviconFile {
  filePath: string
  mimeType: string
}

interface DownloadedFavicon {
  bytes: Buffer
  mimeType: string
}

export type FaviconNetworkSession = Pick<Session, 'fetch' | 'resolveHost' | 'resolveProxy'>

export interface FaviconResourceCacheOptions {
  cacheDirectory?: string
  networkSession: FaviconNetworkSession
  now?: () => number
}

export function registerResourceSchemes(): void {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: RESOURCE_SCHEME,
      privileges: {
        secure: true,
        standard: true,
        supportFetchAPI: true
      }
    }
  ])
}

export class FaviconResourceCache {
  private readonly pending = new Map<string, Promise<ResourceFaviconResponse>>()
  private readonly negativeFailures = new Map<string, number>()
  private readonly networkResolutionWaiters: Array<() => void> = []
  private readonly cacheRootDirectory: string | undefined
  private readonly networkSession: FaviconNetworkSession
  private readonly now: () => number
  private activeNetworkResolutions = 0
  private cacheGeneration = 0
  private mutationQueue: Promise<void> = Promise.resolve()

  constructor(options: FaviconResourceCacheOptions) {
    this.cacheRootDirectory = options.cacheDirectory
    this.networkSession = options.networkSession
    this.now = options.now ?? Date.now
  }

  registerProtocol(): void {
    protocol.handle(RESOURCE_SCHEME, async (request) => this.handleProtocolRequest(request.url))
    void this.enqueueMutation(() => this.pruneCache()).catch(() => undefined)
  }

  async clear(): Promise<void> {
    this.cacheGeneration += 1
    this.pending.clear()
    this.negativeFailures.clear()
    await this.enqueueMutation(() =>
      rm(this.cacheDirectoryPath(), { force: true, recursive: true })
    )
  }

  async resolveFavicon(input: ResourceFaviconRequest): Promise<ResourceFaviconResponse> {
    const pageUrl = normalizeHttpUrl(input.pageUrl)
    if (!pageUrl) return { url: null }

    const cacheKey = cacheKeyForPage(pageUrl)
    const cached = await this.findCachedFile(cacheKey)
    if (cached) {
      this.negativeFailures.delete(cacheKey)
      return { url: faviconProtocolUrl(cacheKey) }
    }

    const pending = this.pending.get(cacheKey)
    if (pending) return pending
    if (this.hasFreshNegativeFailure(cacheKey)) return { url: null }

    const generation = this.cacheGeneration
    const task: Promise<ResourceFaviconResponse> = this.withNetworkResolutionPermit(async () => {
      if (generation !== this.cacheGeneration) return { url: null }

      const result = await this.resolveAndCacheFavicon(
        cacheKey,
        pageUrl,
        input.faviconUrl,
        generation
      )
      if (generation !== this.cacheGeneration) return { url: null }

      if (result.url) {
        this.negativeFailures.delete(cacheKey)
      } else {
        this.rememberNegativeFailure(cacheKey)
      }
      return result
    })
      .catch(() => {
        if (generation === this.cacheGeneration) this.rememberNegativeFailure(cacheKey)
        return { url: null }
      })
      .finally(() => {
        if (this.pending.get(cacheKey) === task) this.pending.delete(cacheKey)
      })

    this.pending.set(cacheKey, task)
    return task
  }

  private async resolveAndCacheFavicon(
    cacheKey: string,
    pageUrl: URL,
    rawFaviconUrl: string | null | undefined,
    generation: number
  ): Promise<ResourceFaviconResponse> {
    const attempted = new Set<string>()
    const cacheCandidate = async (candidate: URL | null): Promise<boolean> => {
      if (
        generation !== this.cacheGeneration ||
        !candidate ||
        attempted.has(candidate.toString())
      ) {
        return false
      }
      attempted.add(candidate.toString())

      const favicon = await fetchFavicon(candidate, this.networkSession).catch(() => null)
      if (!favicon || generation !== this.cacheGeneration) return false

      return this.enqueueMutation(async () => {
        if (generation !== this.cacheGeneration) return false
        await this.writeCachedFile(cacheKey, favicon)
        return true
      })
    }

    if (await cacheCandidate(normalizeHttpUrl(rawFaviconUrl))) {
      return { url: faviconProtocolUrl(cacheKey) }
    }
    if (generation !== this.cacheGeneration) return { url: null }

    const htmlCandidates = await fetchHtmlFaviconCandidates(pageUrl, this.networkSession).catch(
      () => []
    )
    if (generation !== this.cacheGeneration) return { url: null }
    for (const candidate of htmlCandidates) {
      if (generation !== this.cacheGeneration) return { url: null }
      if (await cacheCandidate(candidate)) return { url: faviconProtocolUrl(cacheKey) }
    }

    if (generation !== this.cacheGeneration) return { url: null }
    return (await cacheCandidate(originFaviconUrl(pageUrl)))
      ? { url: faviconProtocolUrl(cacheKey) }
      : { url: null }
  }

  private async handleProtocolRequest(rawUrl: string): Promise<Response> {
    const url = new URL(rawUrl)
    if (url.hostname !== FAVICON_HOST) {
      return new Response(null, { status: 404 })
    }

    const cacheKey = decodeURIComponent(url.pathname.replace(/^\/+/, '')).trim()
    if (!/^[a-f0-9]{64}$/.test(cacheKey)) {
      return new Response(null, { status: 404 })
    }

    const cached = await this.findCachedFile(cacheKey)
    if (!cached) {
      return new Response(null, { status: 404 })
    }

    const bytes = await readFile(cached.filePath)
    return new Response(bytes, {
      headers: {
        'cache-control': 'public, max-age=31536000, immutable',
        'content-type': cached.mimeType
      }
    })
  }

  private async cacheDirectory(): Promise<string> {
    const directory = this.cacheDirectoryPath()
    await mkdir(directory, { recursive: true })
    return directory
  }

  private cacheDirectoryPath(): string {
    return (
      this.cacheRootDirectory ??
      join(app.getPath('userData'), 'resource-cache', FAVICON_CACHE_DIRECTORY)
    )
  }

  private async findCachedFile(cacheKey: string): Promise<CachedFaviconFile | null> {
    const directory = await this.cacheDirectory()
    for (const extension of FAVICON_EXTENSIONS) {
      const filePath = join(directory, `${cacheKey}.${extension}`)
      const stats = await stat(filePath).catch(() => null)
      if (stats?.isFile()) {
        if (stats.mtimeMs < Date.now() - FAVICON_CACHE_MAX_AGE_MS) {
          await rm(filePath, { force: true })
          continue
        }
        return {
          filePath,
          mimeType: MIME_BY_EXTENSION[`.${extension}`] ?? 'application/octet-stream'
        }
      }
    }
    return null
  }

  private async writeCachedFile(cacheKey: string, favicon: DownloadedFavicon): Promise<void> {
    const directory = await this.cacheDirectory()
    await Promise.all(
      FAVICON_EXTENSIONS.map((extension) =>
        rm(join(directory, `${cacheKey}.${extension}`), { force: true })
      )
    )

    const extension = EXTENSION_BY_MIME[favicon.mimeType] ?? 'ico'
    await writeFile(join(directory, `${cacheKey}.${extension}`), favicon.bytes)
    await this.pruneCache()
  }

  private async pruneCache(): Promise<void> {
    const directory = await this.cacheDirectory()
    const entries = await readdir(directory, { withFileTypes: true })
    const files = (
      await Promise.all(
        entries
          .filter((entry) => entry.isFile())
          .map(async (entry) => {
            const filePath = join(directory, entry.name)
            const stats = await stat(filePath).catch(() => null)
            return stats ? { filePath, mtimeMs: stats.mtimeMs } : null
          })
      )
    )
      .filter((file): file is { filePath: string; mtimeMs: number } => Boolean(file))
      .sort((left, right) => right.mtimeMs - left.mtimeMs)
    const cutoff = Date.now() - FAVICON_CACHE_MAX_AGE_MS
    await Promise.all(
      files
        .filter((file, index) => index >= FAVICON_CACHE_MAX_FILES || file.mtimeMs < cutoff)
        .map((file) => rm(file.filePath, { force: true }))
    )
  }

  private enqueueMutation<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.mutationQueue.catch(() => undefined).then(operation)
    this.mutationQueue = result.then(
      () => undefined,
      () => undefined
    )
    return result
  }

  private hasFreshNegativeFailure(cacheKey: string): boolean {
    const expiresAt = this.negativeFailures.get(cacheKey)
    if (expiresAt === undefined) return false
    if (expiresAt <= this.now()) {
      this.negativeFailures.delete(cacheKey)
      return false
    }
    return true
  }

  private rememberNegativeFailure(cacheKey: string): void {
    const now = this.now()
    for (const [key, expiresAt] of this.negativeFailures) {
      if (expiresAt <= now) this.negativeFailures.delete(key)
    }

    this.negativeFailures.delete(cacheKey)
    while (this.negativeFailures.size >= FAVICON_NEGATIVE_CACHE_MAX_ENTRIES) {
      const oldestCacheKey = this.negativeFailures.keys().next().value
      if (oldestCacheKey === undefined) break
      this.negativeFailures.delete(oldestCacheKey)
    }
    this.negativeFailures.set(cacheKey, now + FAVICON_NEGATIVE_CACHE_TTL_MS)
  }

  private async withNetworkResolutionPermit<T>(operation: () => Promise<T>): Promise<T> {
    await this.acquireNetworkResolutionPermit()
    try {
      return await operation()
    } finally {
      this.releaseNetworkResolutionPermit()
    }
  }

  private acquireNetworkResolutionPermit(): Promise<void> {
    if (this.activeNetworkResolutions < FAVICON_MAX_CONCURRENT_RESOLUTIONS) {
      this.activeNetworkResolutions += 1
      return Promise.resolve()
    }

    return new Promise<void>((resolve) => {
      this.networkResolutionWaiters.push(resolve)
    })
  }

  private releaseNetworkResolutionPermit(): void {
    const next = this.networkResolutionWaiters.shift()
    if (next) {
      next()
      return
    }

    this.activeNetworkResolutions -= 1
  }
}

async function fetchHtmlFaviconCandidates(
  pageUrl: URL,
  networkSession: FaviconNetworkSession
): Promise<URL[]> {
  const response = await fetchPublicUrl(
    pageUrl,
    {
      accept: 'text/html,application/xhtml+xml'
    },
    networkSession
  )
  if (!response?.ok) return []
  if (!isHtmlContentType(response.headers.get('content-type'))) return []

  const html = (await readResponseBytes(response, FAVICON_HTML_MAX_BYTES)).toString('utf8')
  return htmlFaviconCandidates(html, pageUrl)
}

async function fetchFavicon(
  url: URL,
  networkSession: FaviconNetworkSession
): Promise<DownloadedFavicon | null> {
  const response = await fetchPublicUrl(
    url,
    {
      accept: 'image/avif,image/webp,image/png,image/svg+xml,image/*,*/*;q=0.8'
    },
    networkSession
  )
  if (!response?.ok) return null

  const mimeType =
    normalizeFaviconMimeType(response.headers.get('content-type')) ?? faviconMimeTypeFromUrl(url)
  if (!mimeType) return null

  const bytes = await readResponseBytes(response, FAVICON_MAX_BYTES)
  if (bytes.length === 0 || bytes.length > FAVICON_MAX_BYTES) return null

  return { bytes, mimeType }
}

async function fetchPublicUrl(
  url: URL,
  options: { accept: string },
  networkSession: FaviconNetworkSession
): Promise<Response | null> {
  let current = new URL(url.toString())

  for (let redirectCount = 0; redirectCount <= FAVICON_MAX_REDIRECTS; redirectCount += 1) {
    if (!(await isPublicHttpUrl(current, networkSession))) return null

    const response = await fetchWithTimeout(networkSession, current, options.accept).catch(
      () => null
    )
    if (!response) return null

    if (response.status >= 300 && response.status < 400) {
      const location = response.headers.get('location')
      if (!location) return null
      current = new URL(location, current)
      continue
    }

    return response
  }

  return null
}

async function fetchWithTimeout(
  networkSession: FaviconNetworkSession,
  url: URL,
  accept: string
): Promise<Response> {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), FAVICON_FETCH_TIMEOUT_MS)
  try {
    return await networkSession.fetch(url.toString(), {
      cache: 'no-store',
      credentials: 'omit',
      headers: {
        accept,
        'user-agent': FAVICON_USER_AGENT
      },
      referrerPolicy: 'no-referrer',
      redirect: 'manual',
      signal: controller.signal
    })
  } finally {
    clearTimeout(timeout)
  }
}

async function readResponseBytes(response: Response, maxBytes: number): Promise<Buffer> {
  const contentLength = Number(response.headers.get('content-length') ?? '')
  if (Number.isFinite(contentLength) && contentLength > maxBytes) {
    throw new Error('Resource is too large')
  }

  const reader = response.body?.getReader()
  if (!reader) return Buffer.alloc(0)

  const chunks: Uint8Array[] = []
  let totalLength = 0

  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    if (!value) continue

    totalLength += value.byteLength
    if (totalLength > maxBytes) {
      await reader.cancel().catch(() => undefined)
      throw new Error('Resource is too large')
    }
    chunks.push(value)
  }

  return Buffer.concat(
    chunks.map((chunk) => Buffer.from(chunk)),
    totalLength
  )
}

async function isPublicHttpUrl(url: URL, networkSession: FaviconNetworkSession): Promise<boolean> {
  if (!['http:', 'https:'].includes(url.protocol)) return false
  if (url.username || url.password) return false

  const host = normalizedHostname(url)
  if (!host) return false
  if (host === 'localhost' || host.endsWith('.localhost')) return false
  if (host === 'metadata.google.internal') return false

  if (isIP(host)) {
    return !isBlockedIpAddress(host)
  }

  const resolved = await networkSession
    .resolveHost(host, {
      cacheUsage: 'disallowed',
      source: 'any',
      secureDnsPolicy: 'allow'
    })
    .catch(() => null)
  const addresses = [
    ...new Set(
      (resolved?.endpoints ?? [])
        .map((endpoint) => endpoint.address)
        .filter((address) => isIP(address) !== 0)
    )
  ]
  if (addresses.length === 0) return false

  const blockedAddresses = addresses.filter(isBlockedIpAddress)
  if (blockedAddresses.length === 0) return true
  if (
    !isProxyFakeIpEligibleUrl(url, host) ||
    blockedAddresses.some((address) => !isProxyFakeIpAddress(address))
  ) {
    return false
  }

  const proxy = await networkSession.resolveProxy(url.toString()).catch(() => '')
  return isExclusiveProxyRoute(proxy)
}

function normalizeHttpUrl(value: string | null | undefined): URL | null {
  if (!value?.trim()) return null

  try {
    const url = new URL(value.trim())
    if (!['http:', 'https:'].includes(url.protocol)) return null
    if (url.username || url.password) return null
    return url
  } catch {
    return null
  }
}

function normalizedHostname(url: URL): string {
  return url.hostname.replace(/^\[|\]$/g, '').toLowerCase()
}

function isBlockedIpAddress(value: string): boolean {
  return classifyAddress(value) !== 'public'
}

function isProxyFakeIpAddress(value: string): boolean {
  const family = isIP(value)
  if (family === 4) return PROXY_FAKE_IP_RANGES.check(value, 'ipv4')
  if (family === 6) return PROXY_FAKE_IP_RANGES.check(value, 'ipv6')
  return false
}

function isProxyFakeIpEligibleUrl(url: URL, host: string): boolean {
  if (url.protocol !== 'https:' || (url.port && url.port !== '443')) return false
  if (!host.includes('.') || host.endsWith('.')) return false

  const deniedSuffixes = [
    'localhost',
    'local',
    'localdomain',
    'home',
    'home.arpa',
    'internal',
    'intranet',
    'lan',
    'corp',
    'invalid',
    'test',
    'example',
    'example.com',
    'example.net',
    'example.org',
    'onion',
    'arpa'
  ]
  if (deniedSuffixes.some((suffix) => host === suffix || host.endsWith(`.${suffix}`))) {
    return false
  }

  return host
    .split('.')
    .every(
      (label) =>
        label.length > 0 && label.length <= 63 && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/u.test(label)
    )
}

function isExclusiveProxyRoute(value: string): boolean {
  const routes = value
    .split(';')
    .map((route) => route.trim())
    .filter(Boolean)
  return (
    routes.length > 0 &&
    routes.every((route) => /^(?:PROXY|HTTPS|SOCKS|SOCKS4|SOCKS5|QUIC)\s+\S+$/iu.test(route))
  )
}

function cacheKeyForPage(pageUrl: URL): string {
  return createHash('sha256').update(pageUrl.origin.toLowerCase()).digest('hex')
}

function faviconProtocolUrl(cacheKey: string): string {
  return `${RESOURCE_SCHEME}://${FAVICON_HOST}/${encodeURIComponent(cacheKey)}`
}

function originFaviconUrl(pageUrl: URL): URL | null {
  return normalizeHttpUrl(`${pageUrl.origin}/favicon.ico`)
}

function pushUniqueUrl(candidates: URL[], candidate: URL | null): void {
  if (!candidate) return
  if (candidates.some((existing) => existing.toString() === candidate.toString())) return
  candidates.push(candidate)
}

function isHtmlContentType(contentType: string | null): boolean {
  if (!contentType) return true
  const mimeType = contentType.split(';')[0].trim().toLowerCase()
  return mimeType === 'text/html' || mimeType === 'application/xhtml+xml'
}

function normalizeFaviconMimeType(contentType: string | null): string | null {
  if (!contentType) return null
  const mimeType = contentType.split(';')[0].trim().toLowerCase()
  if (mimeType === 'image/vnd.microsoft.icon') return 'image/x-icon'
  return EXTENSION_BY_MIME[mimeType] ? mimeType : null
}

function faviconMimeTypeFromUrl(url: URL): string | null {
  return MIME_BY_EXTENSION[extname(url.pathname).toLowerCase()] ?? null
}

function htmlFaviconCandidates(html: string, pageUrl: URL): URL[] {
  const candidates: URL[] = []
  const linkPattern = /<link\b[^>]*>/gi
  let match: RegExpExecArray | null

  while ((match = linkPattern.exec(html))) {
    const tag = match[0]
    const rel = htmlAttributeValue(tag, 'rel')
    if (!relHasIcon(rel)) continue

    const href = htmlAttributeValue(tag, 'href')
    if (!href) continue

    pushUniqueUrl(candidates, resolvePublicUrl(pageUrl, href))
  }

  return candidates
}

function relHasIcon(rel: string | null): boolean {
  return Boolean(
    rel
      ?.toLowerCase()
      .split(/\s+/)
      .some((token) =>
        ['icon', 'shortcut icon', 'apple-touch-icon', 'apple-touch-icon-precomposed'].includes(
          token
        )
      )
  )
}

function htmlAttributeValue(tag: string, name: string): string | null {
  const pattern = new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, 'i')
  const match = pattern.exec(tag)
  const value = match?.[1] ?? match?.[2] ?? match?.[3]
  return value ? decodeHtmlAttribute(value) : null
}

function decodeHtmlAttribute(value: string): string {
  return value
    .replace(/&amp;/g, '&')
    .replace(/&#038;/g, '&')
    .replace(/&#38;/g, '&')
    .trim()
}

function resolvePublicUrl(baseUrl: URL, value: string): URL | null {
  try {
    return normalizeHttpUrl(new URL(value, baseUrl).toString())
  } catch {
    return null
  }
}
