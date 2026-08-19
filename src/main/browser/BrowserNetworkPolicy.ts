import { createHmac, randomBytes } from 'node:crypto'
import { lookup } from 'node:dns/promises'
import { isIP } from 'node:net'
import type { Session } from 'electron'

const DEFAULT_DNS_TIMEOUT_MS = 2_000
const MAX_RESOLVED_ADDRESSES = 16
const MAX_CONCURRENT_RESOLUTIONS = 32
const MAX_URL_BYTES = 8 * 1024

// Must match the Rust BrowserRiskKind declaration/Ord order. Wire validation deliberately rejects
// locale/alphabetical ordering so policy identities remain byte-for-byte stable across runtimes.
const BROWSER_RISK_RANK: Readonly<Record<BrowserRiskKind, number>> = {
  insecure_http: 0,
  localhost: 1,
  loopback: 2,
  private_network: 3,
  link_local: 4,
  cloud_metadata: 5,
  non_standard_port: 6,
  url_userinfo: 7,
  dns_private_resolution: 8,
  risk_escalation: 9,
  new_window: 10,
  file_upload: 11,
  file_download: 12,
  local_service_request: 13
}

export type BrowserResolvedAddressClass =
  'public' | 'loopback' | 'private' | 'link_local' | 'cloud_metadata' | 'unresolved'

export type BrowserRiskKind =
  | 'insecure_http'
  | 'localhost'
  | 'loopback'
  | 'private_network'
  | 'link_local'
  | 'cloud_metadata'
  | 'non_standard_port'
  | 'url_userinfo'
  | 'dns_private_resolution'
  | 'risk_escalation'
  | 'new_window'
  | 'file_upload'
  | 'file_download'
  | 'local_service_request'

export type BrowserHostBoundaryCode =
  | 'unsupported_scheme'
  | 'main_renderer'
  | 'privileged_electron'
  | 'internal_debug'
  | 'mcp_control'
  | 'resolution_overflow'
  | 'resolution_unavailable'

export interface BrowserDnsResolver {
  resolve(hostname: string, signal: AbortSignal): Promise<readonly string[]>
}

export interface BrowserDestinationIdentity {
  /** Query, fragment, and userinfo are omitted so the value is safe for approval UI and logs. */
  displayUrl: string
  origin: string
  scheme: 'http' | 'https'
  host: string
  port: number
  addressClass: BrowserResolvedAddressClass
  /** Binds approval to the exact URL without exposing credentials or query values. */
  targetDigest: string
  /** Binds approval to the exact DNS answer set without exposing addresses to the Renderer. */
  resolutionFingerprint: string
}

export type BrowserDestinationAssessment =
  | {
      disposition: 'allow'
      destination: BrowserDestinationIdentity
      riskKinds: readonly []
    }
  | {
      disposition: 'approval_required'
      destination: BrowserDestinationIdentity
      riskKinds: readonly BrowserRiskKind[]
    }
  | {
      disposition: 'deny'
      code: BrowserHostBoundaryCode
    }

export interface BrowserDestinationAssessmentOptions {
  additionalRisks?: readonly BrowserRiskKind[]
  /** Added only when the destination already crosses another network risk boundary. */
  contextualRisks?: readonly BrowserRiskKind[]
  signal?: AbortSignal
}

export interface BrowserNetworkPolicyOptions {
  blockedOrigins?: readonly string[]
  debugEndpoints?: readonly { host: string; port: number }[]
  dnsResolver?: BrowserDnsResolver
  dnsTimeoutMs?: number
  mcpControlEndpoints?: readonly { host: string; port: number }[]
}

/**
 * Main-owned network classifier for the managed Browser guest.
 *
 * The classifier does not grant access. It only separates ordinary public HTTPS, reviewable
 * destinations, and non-reviewable Host boundaries. A BrowserRiskGrant is issued by Rust after a
 * typed approval; Main must never infer approval from this result.
 */
export class BrowserNetworkPolicy {
  private readonly blockedOrigins: ReadonlySet<string>
  private readonly rendererEndpoints: ReadonlySet<string>
  private readonly debugEndpoints: ReadonlySet<string>
  private readonly dnsResolver: BrowserDnsResolver
  private readonly dnsTimeoutMs: number
  private readonly mcpControlEndpoints: ReadonlySet<string>
  private readonly identityKey = randomBytes(32)
  private readonly pendingResolutions = new Map<
    string,
    { controller: AbortController; observable: Promise<ResolutionResult> }
  >()
  private disposed = false

  constructor(options: BrowserNetworkPolicyOptions = {}) {
    const blockedOrigins = (options.blockedOrigins ?? [])
      .map(normalizeConfiguredOrigin)
      .filter(isPresent)
    this.blockedOrigins = new Set(blockedOrigins.map((entry) => entry.origin))
    this.rendererEndpoints = new Set(
      blockedOrigins.flatMap((entry) => rendererEndpointKeys(entry.host, entry.port))
    )
    this.debugEndpoints = new Set((options.debugEndpoints ?? []).map(endpointKey))
    this.dnsResolver = options.dnsResolver ?? new NodeBrowserDnsResolver()
    this.dnsTimeoutMs = normalizeTimeout(options.dnsTimeoutMs)
    this.mcpControlEndpoints = new Set((options.mcpControlEndpoints ?? []).map(endpointKey))
  }

  async assess(
    untrustedUrl: string,
    options: BrowserDestinationAssessmentOptions = {}
  ): Promise<BrowserDestinationAssessment> {
    if (this.disposed) return { disposition: 'deny', code: 'resolution_unavailable' }
    if (typeof untrustedUrl !== 'string' || byteLength(untrustedUrl) > MAX_URL_BYTES) {
      return { disposition: 'deny', code: 'unsupported_scheme' }
    }

    let url: URL
    try {
      url = new URL(untrustedUrl)
    } catch {
      return { disposition: 'deny', code: 'unsupported_scheme' }
    }

    const schemeBoundary = classifySchemeBoundary(url.protocol)
    if (schemeBoundary) return { disposition: 'deny', code: schemeBoundary }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return { disposition: 'deny', code: 'unsupported_scheme' }
    }

    const hostname = normalizeHostname(url.hostname)
    const port = effectivePort(url)
    const endpoint = endpointKey({ host: hostname, port })
    if (this.blockedOrigins.has(url.origin)) {
      return { disposition: 'deny', code: 'main_renderer' }
    }
    if (this.debugEndpoints.has(endpoint)) {
      return { disposition: 'deny', code: 'internal_debug' }
    }
    if (this.mcpControlEndpoints.has(endpoint)) {
      return { disposition: 'deny', code: 'mcp_control' }
    }

    const resolved = await this.resolve(hostname, options.signal)
    if (resolved.overflow) return { disposition: 'deny', code: 'resolution_overflow' }
    if (resolved.failed) return { disposition: 'deny', code: 'resolution_unavailable' }
    const boundary = this.classifyResolvedBoundary(resolved.addresses, port)
    if (boundary) return { disposition: 'deny', code: boundary }
    const hostnameClass = classifyHostname(hostname)
    const addressClasses = resolved.addresses.map(classifyAddress)
    const addressClass = strongestAddressClass(hostnameClass, addressClasses, resolved.failed)
    const risks = new Set<BrowserRiskKind>(options.additionalRisks ?? [])

    if (url.protocol === 'http:') risks.add('insecure_http')
    if (!isStandardPort(url.protocol, port)) risks.add('non_standard_port')
    if (url.username.length > 0 || url.password.length > 0) risks.add('url_userinfo')
    appendAddressRisks(risks, hostnameClass, addressClass, isIP(hostname) === 0)
    if (risks.size > 0) {
      for (const risk of options.contextualRisks ?? []) risks.add(risk)
    }

    const destination: BrowserDestinationIdentity = {
      displayUrl: safeDisplayUrl(url),
      origin: url.origin,
      scheme: url.protocol === 'https:' ? 'https' : 'http',
      host: hostname,
      port,
      addressClass,
      targetDigest: this.fingerprint(normalizeSecurityUrl(url)),
      resolutionFingerprint: this.fingerprint(
        resolved.addresses.length > 0
          ? [...new Set(resolved.addresses.map(normalizeIpLiteral))].sort().join('\n')
          : `unresolved:${hostname}`
      )
    }
    const riskKinds = [...risks].sort(compareRisks)
    return riskKinds.length === 0
      ? { disposition: 'allow', destination, riskKinds: [] }
      : { disposition: 'approval_required', destination, riskKinds }
  }

  private async resolve(hostname: string, callerSignal?: AbortSignal): Promise<ResolutionResult> {
    if (isIP(hostname) !== 0) {
      return { addresses: [hostname], failed: false, overflow: false }
    }

    let pending = this.pendingResolutions.get(hostname)
    if (!pending) {
      if (this.pendingResolutions.size >= MAX_CONCURRENT_RESOLUTIONS) {
        return { addresses: [], failed: true, overflow: false }
      }
      const controller = new AbortController()
      const timer = setTimeout(() => controller.abort('dns_timeout'), this.dnsTimeoutMs)
      const created = { controller } as {
        controller: AbortController
        observable: Promise<ResolutionResult>
      }
      const raw = Promise.resolve()
        .then(() => this.dnsResolver.resolve(hostname, controller.signal))
        .then(normalizeResolution, () => ({ addresses: [], failed: true, overflow: false }))
        .finally(() => {
          clearTimeout(timer)
          if (this.pendingResolutions.get(hostname) === created) {
            this.pendingResolutions.delete(hostname)
          }
        })
      const observable = raceWithAbort(raw, controller.signal)
      // A resolver may ignore AbortSignal. Keep its physical slot until `raw` settles, but consume
      // the bounded observable rejection so late DNS work cannot become an unhandled rejection.
      void observable.catch(() => undefined)
      created.observable = observable
      this.pendingResolutions.set(hostname, created)
      pending = created
    }
    try {
      return callerSignal
        ? await raceWithAbort(pending.observable, callerSignal)
        : await pending.observable
    } catch {
      if (callerSignal?.aborted) throw new Error('DNS resolution cancelled')
      return { addresses: [], failed: true, overflow: false }
    }
  }

  shutdown(): void {
    if (this.disposed) return
    this.disposed = true
    for (const pending of this.pendingResolutions.values()) pending.controller.abort('shutdown')
  }

  snapshot(): { disposed: boolean; pendingResolutions: number } {
    return { disposed: this.disposed, pendingResolutions: this.pendingResolutions.size }
  }

  private classifyResolvedBoundary(
    addresses: readonly string[],
    port: number
  ): BrowserHostBoundaryCode | null {
    for (const address of addresses) {
      const key = endpointKey({ host: address, port })
      if (this.rendererEndpoints.has(key)) return 'main_renderer'
      if (this.debugEndpoints.has(key)) return 'internal_debug'
      if (this.mcpControlEndpoints.has(key)) return 'mcp_control'
    }
    return null
  }

  private fingerprint(value: string): string {
    return `hmac-sha256:${createHmac('sha256', this.identityKey).update(value, 'utf8').digest('hex')}`
  }
}

export class NodeBrowserDnsResolver implements BrowserDnsResolver {
  async resolve(hostname: string, signal: AbortSignal): Promise<readonly string[]> {
    if (signal.aborted) throw new Error('DNS resolution cancelled')
    const resolved = await lookup(hostname, { all: true, verbatim: true })
    if (signal.aborted) throw new Error('DNS resolution cancelled')
    return resolved.map((entry) => entry.address)
  }
}

/** Uses the managed guest's Chromium NetworkContext rather than Node's independent resolver. */
export class ElectronSessionDnsResolver implements BrowserDnsResolver {
  constructor(private readonly managedSession: Pick<Session, 'resolveHost'>) {}

  async resolve(hostname: string, signal: AbortSignal): Promise<readonly string[]> {
    if (signal.aborted) throw new Error('DNS resolution cancelled')
    const result = await this.managedSession.resolveHost(hostname, {
      cacheUsage: 'disallowed',
      source: 'any',
      secureDnsPolicy: 'allow'
    })
    if (signal.aborted) throw new Error('DNS resolution cancelled')
    return result.endpoints.map((endpoint) => endpoint.address)
  }
}

interface ResolutionResult {
  addresses: readonly string[]
  failed: boolean
  overflow: boolean
}

function normalizeResolution(addresses: readonly string[]): ResolutionResult {
  const validAddresses = [...new Set(addresses.filter((address) => isIP(address) !== 0))]
  return {
    addresses: validAddresses.slice(0, MAX_RESOLVED_ADDRESSES),
    failed: validAddresses.length === 0,
    overflow: validAddresses.length > MAX_RESOLVED_ADDRESSES
  }
}

function classifySchemeBoundary(protocol: string): BrowserHostBoundaryCode | null {
  switch (protocol) {
    case 'chrome:':
    case 'chrome-extension:':
    case 'devtools:':
    case 'file:':
    case 'javascript:':
      return 'privileged_electron'
    default:
      return null
  }
}

function classifyHostname(hostname: string): BrowserResolvedAddressClass | null {
  const normalized = hostname.toLowerCase().replace(/\.$/, '')
  if (normalized === 'localhost' || normalized.endsWith('.localhost')) return 'loopback'
  if (isCloudMetadataHostname(normalized)) return 'cloud_metadata'
  return null
}

export function classifyAddress(address: string): BrowserResolvedAddressClass {
  const normalized = normalizeIpLiteral(address)
  const family = isIP(normalized)
  if (family === 4) return classifyIpv4(normalized)
  if (family === 6) return classifyIpv6(normalized)
  return 'unresolved'
}

function classifyIpv4(address: string): BrowserResolvedAddressClass {
  const octets = address.split('.').map(Number)
  const [a = -1, b = -1, c = -1, d = -1] = octets
  if (isCloudMetadataIpv4(a, b, c, d)) return 'cloud_metadata'
  if (a === 127 || a === 0) return 'loopback'
  if (a === 169 && b === 254) return 'link_local'
  if (
    a === 10 ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 100 && b >= 64 && b <= 127) ||
    (a === 198 && (b === 18 || b === 19)) ||
    a >= 224
  ) {
    return 'private'
  }
  return 'public'
}

function classifyIpv6(address: string): BrowserResolvedAddressClass {
  const bytes = ipv6Bytes(address)
  if (!bytes) return 'unresolved'
  if (bytes.every((value, index) => value === (index === 15 ? 1 : 0))) return 'loopback'
  if (bytes.every((value) => value === 0)) return 'loopback'
  if (isCloudMetadataIpv6(bytes)) return 'cloud_metadata'
  if (bytes[0] === 0xfe && (bytes[1]! & 0xc0) === 0x80) return 'link_local'
  if ((bytes[0]! & 0xfe) === 0xfc || bytes[0] === 0xff) return 'private'

  const ipv4 = embeddedIpv4Address(bytes)
  return ipv4 ? classifyIpv4(ipv4) : 'public'
}

function strongestAddressClass(
  hostnameClass: BrowserResolvedAddressClass | null,
  addressClasses: readonly BrowserResolvedAddressClass[],
  resolutionFailed: boolean
): BrowserResolvedAddressClass {
  const values = hostnameClass ? [hostnameClass, ...addressClasses] : [...addressClasses]
  const rank: Record<BrowserResolvedAddressClass, number> = {
    public: 0,
    unresolved: 1,
    private: 2,
    link_local: 3,
    loopback: 4,
    cloud_metadata: 5
  }
  if (values.length === 0) return resolutionFailed ? 'unresolved' : 'public'
  return values.reduce((strongest, value) => (rank[value] > rank[strongest] ? value : strongest))
}

function appendAddressRisks(
  risks: Set<BrowserRiskKind>,
  hostnameClass: BrowserResolvedAddressClass | null,
  addressClass: BrowserResolvedAddressClass,
  wasDnsName: boolean
): void {
  switch (addressClass) {
    case 'cloud_metadata':
      risks.add('cloud_metadata')
      risks.add('local_service_request')
      break
    case 'loopback':
      if (hostnameClass === 'loopback') risks.add('localhost')
      risks.add('loopback')
      risks.add('local_service_request')
      break
    case 'link_local':
      risks.add('link_local')
      risks.add('local_service_request')
      break
    case 'private':
      risks.add('private_network')
      risks.add('local_service_request')
      break
    case 'unresolved':
      // An unresolved hostname cannot be proven public. Treat it as reviewable rather than silently
      // allowing Chromium to resolve a private address after this check.
      risks.add('dns_private_resolution')
      break
    case 'public':
      break
  }
  if (
    wasDnsName &&
    hostnameClass === null &&
    addressClass !== 'public' &&
    addressClass !== 'unresolved'
  ) {
    risks.add('dns_private_resolution')
  }
}

function safeDisplayUrl(url: URL): string {
  const safe = new URL(url.toString())
  safe.username = ''
  safe.password = ''
  safe.search = ''
  safe.hash = ''
  const value = safe.toString()
  return value.length <= 2_048 ? value : `${value.slice(0, 2_045)}...`
}

function normalizeSecurityUrl(url: URL): string {
  const normalized = new URL(url.toString())
  normalized.hash = ''
  return normalized.toString()
}

function normalizeConfiguredOrigin(
  value: string
): { origin: string; host: string; port: number } | null {
  try {
    const url = new URL(value)
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
    return {
      origin: url.origin,
      host: normalizeHostname(url.hostname),
      port: effectivePort(url)
    }
  } catch {
    return null
  }
}

function effectivePort(url: URL): number {
  if (url.port.length > 0) return Number(url.port)
  return url.protocol === 'https:' ? 443 : 80
}

function isStandardPort(protocol: string, port: number): boolean {
  return (protocol === 'https:' && port === 443) || (protocol === 'http:' && port === 80)
}

function endpointKey(value: { host: string; port: number }): string {
  const hostname = normalizeHostname(value.host)
  const bytes = isIP(hostname) === 6 ? ipv6Bytes(hostname) : null
  return `${(bytes && embeddedIpv4Address(bytes)) || hostname}:${value.port}`
}

function normalizeHostname(value: string): string {
  const withoutBrackets = value.startsWith('[') && value.endsWith(']') ? value.slice(1, -1) : value
  return withoutBrackets.toLowerCase().replace(/\.$/, '')
}

function rendererEndpointKeys(host: string, port: number): string[] {
  const normalized = normalizeHostname(host)
  if (normalized === 'localhost' || normalized.endsWith('.localhost')) {
    return [
      endpointKey({ host: normalized, port }),
      endpointKey({ host: '127.0.0.1', port }),
      endpointKey({ host: '::1', port })
    ]
  }
  return [endpointKey({ host: normalized, port })]
}

function isCloudMetadataHostname(hostname: string): boolean {
  return (
    hostname === 'metadata.google.internal' ||
    hostname === 'metadata.google' ||
    hostname === 'instance-data' ||
    hostname === 'metadata.azure.internal'
  )
}

function isCloudMetadataIpv4(a: number, b: number, c: number, d: number): boolean {
  return (
    (a === 169 && b === 254 && c === 169 && (d === 254 || d === 123)) ||
    (a === 169 && b === 254 && c === 170 && d === 2) ||
    (a === 100 && b === 100 && c === 100 && d === 200) ||
    (a === 168 && b === 63 && c === 129 && d === 16) ||
    (a === 192 && b === 0 && c === 0 && d === 192)
  )
}

function isCloudMetadataIpv6(bytes: readonly number[]): boolean {
  // AWS fd00:ec2::254 and IPv6-mapped/link-local metadata endpoints.
  return (
    bytes[0] === 0xfd &&
    bytes[1] === 0x00 &&
    bytes[2] === 0x0e &&
    bytes[3] === 0xc2 &&
    bytes.slice(4, 14).every((value) => value === 0) &&
    bytes[14] === 0x02 &&
    bytes[15] === 0x54
  )
}

function normalizeIpLiteral(value: string): string {
  const zone = value.indexOf('%')
  return (zone >= 0 ? value.slice(0, zone) : value).toLowerCase()
}

function ipv6Bytes(value: string): number[] | null {
  let normalized = normalizeIpLiteral(value)
  const lastColon = normalized.lastIndexOf(':')
  const ipv4Tail = normalized.slice(lastColon + 1)
  if (ipv4Tail.includes('.')) {
    if (isIP(ipv4Tail) !== 4) return null
    const octets = ipv4Tail.split('.').map(Number)
    normalized = `${normalized.slice(0, lastColon)}:${((octets[0]! << 8) | octets[1]!).toString(16)}:${((octets[2]! << 8) | octets[3]!).toString(16)}`
  }
  const halves = normalized.split('::')
  if (halves.length > 2) return null
  const left = halves[0] ? halves[0].split(':') : []
  const right = halves[1] ? halves[1].split(':') : []
  const fill = halves.length === 2 ? 8 - left.length - right.length : 0
  const groups = [...left, ...Array.from({ length: fill }, () => '0'), ...right]
  if (groups.length !== 8) return null
  const bytes: number[] = []
  for (const group of groups) {
    if (!/^[0-9a-f]{1,4}$/i.test(group)) return null
    const number = Number.parseInt(group, 16)
    bytes.push(number >>> 8, number & 0xff)
  }
  return bytes
}

function embeddedIpv4Address(bytes: readonly number[]): string | null {
  const isMapped =
    bytes.slice(0, 10).every((value) => value === 0) && bytes[10] === 0xff && bytes[11] === 0xff
  const isCompatible = bytes.slice(0, 12).every((value) => value === 0)
  if (isMapped || isCompatible) {
    return `${bytes[12]}.${bytes[13]}.${bytes[14]}.${bytes[15]}`
  }
  return null
}

function byteLength(value: string): number {
  return Buffer.byteLength(value, 'utf8')
}

function normalizeTimeout(value: number | undefined): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, 10_000)
    : DEFAULT_DNS_TIMEOUT_MS
}

function compareRisks(left: BrowserRiskKind, right: BrowserRiskKind): number {
  return BROWSER_RISK_RANK[left] - BROWSER_RISK_RANK[right]
}

function isPresent<T>(value: T | null): value is T {
  return value !== null
}

async function raceWithAbort<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) throw new Error('Operation cancelled')
  return await new Promise<T>((resolve, reject) => {
    const abort = (): void => reject(new Error('Operation cancelled'))
    signal.addEventListener('abort', abort, { once: true })
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}
