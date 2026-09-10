import type { WebContents } from 'electron'
import { parseBrowserSurfaceBootstrapUrl } from '@mycopilot/protocol'
import type {
  BrowserSensitiveTargetIdentity,
  ManagedSurface,
  PhysicalHistoryEntry
} from './BrowserSurfaceTypes'
import { BrowserSurfaceManagerError } from './BrowserSurfaceTypes'
import type { BrowserSurfaceCrashError } from './BrowserLoadErrorPage'

export const DEFAULT_ATTACH_TIMEOUT_MS = 10_000
export const DEFAULT_CLOSE_TIMEOUT_MS = 2_000
export const MAX_POPUPS_PER_SECOND = 4
export const MAX_PENDING_RENDERER_COMMANDS = 64
export const STRICT_MODE_SURFACE_HANDOFF_MS = 100
export const MAX_SETTLED_SURFACE_REQUESTS = 256
export const INTERNAL_ERROR_PAGE_MAX_ATTEMPTS = 4
export const INTERNAL_ERROR_PAGE_RETRY_DELAY_MS = 40
export const INTERNAL_ERROR_PAGE_LOAD_TIMEOUT_MS = 750
export const MAX_INTERNAL_DOCUMENTS_PER_SURFACE = 64

const DEFAULT_MAX_MANAGED_SURFACES = 8
const MAX_CONFIGURED_MANAGED_SURFACES = 16

export class InternalPageLoadAttemptTimeoutError extends Error {}

export function normalizeTimeout(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, 60_000)
    : fallback
}

export function normalizeSurfaceCapacity(value: number | undefined): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, MAX_CONFIGURED_MANAGED_SURFACES)
    : DEFAULT_MAX_MANAGED_SURFACES
}

export function normalizeSurfaceIndex(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }
  return value
}

export function normalizeViewportSize(input: { height: number; width: number }): {
  height: number
  width: number
} {
  if (
    !Number.isSafeInteger(input.width) ||
    !Number.isSafeInteger(input.height) ||
    input.width < 240 ||
    input.width > 4_096 ||
    input.height < 240 ||
    input.height > 4_096
  ) {
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }
  return { height: input.height, width: input.width }
}

export function safeSurfaceTitle(value: string): string {
  const normalized = value.replace(/\p{Cc}/gu, ' ').trim()
  return normalized.slice(0, 256) || 'New tab'
}

export function fallbackSurfaceTitle(value: string): string {
  try {
    const parsed = new URL(value)
    if (parsed.protocol === 'file:') return fileUrlDisplayName(parsed).slice(0, 256)
    return parsed.hostname.slice(0, 256) || 'New tab'
  } catch {
    return 'New tab'
  }
}

export function isNavigableBrowserProtocol(protocol: string): boolean {
  return protocol === 'http:' || protocol === 'https:' || protocol === 'file:'
}

export function fileUrlDisplayName(url: URL): string {
  const encoded = url.pathname.split('/').filter(Boolean).at(-1) ?? ''
  let name = encoded
  try {
    name = decodeURIComponent(encoded)
  } catch {
    // Keep the encoded segment when it is not valid UTF-8 percent-encoding.
  }
  const sanitized = name.replace(/[/\\@]/g, '-').trim()
  return sanitized || 'file'
}

export function historyHostnameForUrl(url: URL): string {
  if (url.protocol !== 'file:') return url.hostname.toLowerCase()
  if (url.hostname) return url.hostname.toLowerCase()
  return fileUrlDisplayName(url).toLowerCase().slice(0, 255)
}

export function safeLogicalSurfaceUrl(value: string): string | null {
  if (!value || value.length > 16_384) return null
  try {
    const parsed = new URL(value)
    if (
      !isNavigableBrowserProtocol(parsed.protocol) ||
      parsed.username !== '' ||
      parsed.password !== ''
    ) {
      return null
    }
    return parsed.toString()
  } catch {
    return null
  }
}

export function safeRemoteResourceUrl(value: string): string | null {
  if (!value || value.length > 4_096) return null
  try {
    const parsed = new URL(value)
    if (
      !['http:', 'https:'].includes(parsed.protocol) ||
      parsed.username !== '' ||
      parsed.password !== ''
    ) {
      return null
    }
    return parsed.toString()
  } catch {
    return null
  }
}

export function isManagedBlankSurfaceUrl(value: string): boolean {
  return value === 'about:blank' || parseBrowserSurfaceBootstrapUrl(value) !== null
}

export function haveSameHttpOrigin(left: string | null, right: string | null): boolean {
  if (!left || !right) return false
  try {
    return new URL(left).origin === new URL(right).origin
  } catch {
    return false
  }
}

export function safeGuestBoolean(guest: WebContents, method: 'isLoadingMainFrame'): boolean {
  try {
    return Boolean(guest[method]())
  } catch {
    return false
  }
}

export function safeGuestHistoryBoolean(
  guest: WebContents,
  method: 'canGoBack' | 'canGoForward'
): boolean {
  try {
    const history = guest.navigationHistory
    if (history && typeof history[method] === 'function') return Boolean(history[method]())
    return Boolean(guest[method]())
  } catch {
    return false
  }
}

export function runGuestHistoryAction(guest: WebContents, action: 'goBack' | 'goForward'): void {
  const history = guest.navigationHistory
  if (history && typeof history[action] === 'function') {
    history[action]()
    return
  }
  guest[action]()
}

interface SafeNavigationHistory {
  getActiveIndex?: () => number
  getAllEntries?: () => Array<{ title?: string; url: string }>
  removeEntryAtIndex?: (index: number) => boolean
}

function safeNavigationHistory(guest: WebContents): SafeNavigationHistory | null {
  try {
    return (guest.navigationHistory as unknown as SafeNavigationHistory) ?? null
  } catch {
    return null
  }
}

export function safeActiveHistoryIndex(guest: WebContents): number {
  try {
    const index = safeNavigationHistory(guest)?.getActiveIndex?.()
    return Number.isSafeInteger(index) && index !== undefined && index >= 0 ? index : -1
  } catch {
    return -1
  }
}

export function safeHistoryEntries(guest: WebContents): Array<{ title?: string; url: string }> {
  try {
    const entries = safeNavigationHistory(guest)?.getAllEntries?.()
    if (!Array.isArray(entries)) return []
    return entries.filter((entry): entry is { title?: string; url: string } =>
      Boolean(entry && typeof entry.url === 'string')
    )
  } catch {
    return []
  }
}

export function safeActiveHistoryEntry(guest: WebContents): PhysicalHistoryEntry | null {
  const index = safeActiveHistoryIndex(guest)
  if (index < 0) return null
  const entry = safeHistoryEntries(guest)[index]
  return entry ? { index, url: entry.url } : null
}

export function safeRemoveHistoryEntry(guest: WebContents, index: number): boolean {
  if (index < 0) return false
  try {
    return safeNavigationHistory(guest)?.removeEntryAtIndex?.(index) === true
  } catch {
    // History compaction is best effort; logical state and internal-page admission remain exact.
    return false
  }
}

export function safeRemoveHistoryEntryByUrl(guest: WebContents, url: string): boolean {
  const activeIndex = safeActiveHistoryIndex(guest)
  const index = safeHistoryEntries(guest).findIndex(
    (entry, candidateIndex) => candidateIndex !== activeIndex && entry.url === url
  )
  return safeRemoveHistoryEntry(guest, index)
}

export function isIgnoredBrowserLoadFailure(errorCode: number, errorDescription: string): boolean {
  const normalized = errorDescription
    .trim()
    .toUpperCase()
    .replace(/^NET::/u, '')
  return errorCode === -3 || normalized === 'ERR_ABORTED' || normalized === 'ERR_BLOCKED_BY_CLIENT'
}

export function browserNavigationErrorDescription(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  return message.match(/(?:ERR_[A-Z0-9_]+|DNS_PROBE_POSSIBLE)/u)?.[0] ?? 'ERR_FAILED'
}

export function isNavigationAlreadyPendingError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return message.toLowerCase().includes('navigation is already pending')
}

export function isAbortedBrowserNavigationError(error: unknown): boolean {
  return browserNavigationErrorDescription(error) === 'ERR_ABORTED'
}

export function fallbackBrowserSurfaceCrashError(input: {
  generation: number
  kind: 'renderer_crashed' | 'renderer_unresponsive'
  navigationEpoch: number
}): BrowserSurfaceCrashError {
  const unresponsive = input.kind === 'renderer_unresponsive'
  const title = unresponsive ? 'Page is not responding' : 'Page renderer stopped'
  return {
    actionLabel: unresponsive ? 'Reload page' : 'Recreate page',
    generation: input.generation,
    heading: title,
    internalActionUrl: '',
    internalPageHtml: '',
    kind: input.kind,
    navigationEpoch: input.navigationEpoch,
    summary: unresponsive
      ? 'The page stopped responding. Reload it to continue.'
      : 'The page renderer exited unexpectedly. Recreate it to continue.',
    title
  }
}

export function normalizeRendererGoneReason(reason: string): string {
  const allowed = new Set([
    'clean-exit',
    'abnormal-exit',
    'killed',
    'crashed',
    'oom',
    'launch-failed',
    'integrity-failure'
  ])
  return allowed.has(reason) ? reason : 'unknown'
}

export function safeSurfaceUrl(value: string): string {
  try {
    const parsed = new URL(value)
    if (!isNavigableBrowserProtocol(parsed.protocol)) return 'about:blank'
    if (parsed.username !== '' || parsed.password !== '') return 'about:blank'
    return parsed.toString().slice(0, 16_384)
  } catch {
    return 'about:blank'
  }
}

export function safeHttpOrigin(value: string): string | null {
  try {
    const parsed = new URL(value)
    if (parsed.username !== '' || parsed.password !== '') return null
    if (parsed.protocol === 'file:') return 'file://'
    if (!['http:', 'https:'].includes(parsed.protocol)) return null
    return parsed.origin
  } catch {
    return null
  }
}

export function sameSensitiveTarget(
  left: BrowserSensitiveTargetIdentity | null,
  right: BrowserSensitiveTargetIdentity
): boolean {
  return Boolean(
    left &&
    left.surfaceId === right.surfaceId &&
    left.generation === right.generation &&
    left.navigationEpoch === right.navigationEpoch &&
    left.origin === right.origin
  )
}

export function isSafeManagedPageUrl(value: string): boolean {
  try {
    return isNavigableBrowserProtocol(new URL(value).protocol)
  } catch {
    return false
  }
}

export async function loadManagedSurface(
  surface: ManagedSurface,
  url: string,
  timeoutMs: number
): Promise<void> {
  if (!isSafeManagedPageUrl(url)) {
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }
  await loadExactManagedSurfaceUrl(surface, url, timeoutMs)
}

export async function loadManagedBlankSurface(
  surface: ManagedSurface,
  timeoutMs: number
): Promise<void> {
  await loadExactManagedSurfaceUrl(surface, 'about:blank', timeoutMs)
}

async function loadExactManagedSurfaceUrl(
  surface: ManagedSurface,
  url: string,
  timeoutMs: number
): Promise<void> {
  await waitForInitialDocumentReady(surface, timeoutMs)
  await waitForInitialNavigationToSettle(surface, timeoutMs)
  if (surface.guest.isDestroyed()) {
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([
      surface.guest.loadURL(url),
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new BrowserSurfaceManagerError('browser.surface_unavailable')),
          timeoutMs
        )
      })
    ])
  } catch (error) {
    if (surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    if (error instanceof BrowserSurfaceManagerError) throw error
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  } finally {
    if (timer) clearTimeout(timer)
  }
}

async function waitForInitialNavigationToSettle(
  surface: ManagedSurface,
  timeoutMs: number
): Promise<void> {
  const settled = await boundedWaitFor(() => {
    if (surface.guest.isDestroyed()) return true
    const nativeMainFrameLoading =
      typeof surface.guest.isLoadingMainFrame === 'function' && surface.guest.isLoadingMainFrame()
    return !surface.navigationInProgress && !nativeMainFrameLoading
  }, timeoutMs)
  if (surface.guest.isDestroyed()) {
    throw new BrowserSurfaceManagerError('browser.target_closed')
  }
  if (!settled) throw new BrowserSurfaceManagerError('browser.surface_unavailable')
}

export async function waitForInitialDocumentReady(
  surface: ManagedSurface,
  timeoutMs: number
): Promise<void> {
  if (surface.initialDocumentReady) return
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    await Promise.race([
      surface.initialDocumentReadyPromise,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new BrowserSurfaceManagerError('browser.surface_unavailable')),
          timeoutMs
        )
      })
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
  if (surface.guest.isDestroyed()) {
    throw new BrowserSurfaceManagerError('browser.target_closed')
  }
}

export async function boundedWaitFor(
  predicate: () => boolean,
  timeoutMs: number
): Promise<boolean> {
  if (predicate()) return true
  return await new Promise<boolean>((resolve) => {
    const deadline = Date.now() + timeoutMs
    const poll = (): void => {
      if (predicate()) {
        resolve(true)
        return
      }
      if (Date.now() >= deadline) {
        resolve(false)
        return
      }
      setTimeout(poll, 10)
    }
    poll()
  })
}

export async function nextEventLoopTurn(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve))
}

export async function settleWithin<T>(
  promise: Promise<T> | undefined,
  timeoutMs: number
): Promise<boolean> {
  if (!promise) return true
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise.then(
        () => true,
        () => true
      ),
      new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), timeoutMs)
      })
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}
