import { randomUUID } from 'node:crypto'
import type { WebContents } from 'electron'
import type { Browser, BrowserContext } from 'playwright'
import { chromium } from 'playwright'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  BROWSER_WEBVIEW_PARTITION,
  parseBrowserSurfaceBootstrapUrl,
  type BrowserSurfaceCommand,
  type BrowserSurfaceReadyInput,
  type BrowserSurfaceReadyOutput
} from '@mycopilot/protocol'
import { BrowserTargetBroker } from './BrowserTargetBroker'
import type { ElectronGuestCdpTransport } from './ElectronGuestCdpTransport'
import { BrowserNetworkGuard, type BrowserNetworkOperationLease } from './BrowserNetworkGuard'
import type { BrowserRiskOperationInput } from './BrowserRiskCoordinator'

const DEFAULT_ATTACH_TIMEOUT_MS = 10_000
const DEFAULT_CLOSE_TIMEOUT_MS = 2_000
const MAX_MANAGED_SURFACES = 16

export type BrowserSurfaceManagerErrorCode =
  'browser.surface_unavailable' | 'browser.target_closed' | 'browser.manager_shutdown'

export class BrowserSurfaceManagerError extends Error {
  readonly name = 'BrowserSurfaceManagerError'

  constructor(readonly code: BrowserSurfaceManagerErrorCode) {
    super(code)
  }
}

export interface BrowserSurfaceManagerOptions {
  attachTimeoutMs?: number
  broker: BrowserTargetBroker
  networkGuard?: BrowserNetworkGuard
  closeTimeoutMs?: number
  connectOverCdp?: (transport: ElectronGuestCdpTransport) => Promise<Browser>
  resolveHost: () => WebContents | null
  sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
}

interface ManagedSurface {
  generation: number
  guest: WebContents
  handleDestroyed: () => void
  host: WebContents
  surfaceId: string
}

interface ActiveAttachment {
  browser: Browser
  context: BrowserContext
  generation: number
  surfaceId: string
  transport: ElectronGuestCdpTransport
}

interface PendingEnsure {
  host: WebContents
  promise: Promise<ManagedSurface>
  readySurfaceId?: string
  reject: (error: BrowserSurfaceManagerError) => void
  requestId: string
  resolve: (surface: ManagedSurface) => void
  settled: boolean
  timer: ReturnType<typeof setTimeout>
}

interface PendingClose {
  generation: number
  reject: (error: BrowserSurfaceManagerError) => void
  resolve: () => void
  timer: ReturnType<typeof setTimeout>
}

/**
 * Main-owned coordinator for the one browser guest exposed to managed automation.
 *
 * The Renderer can ask for a visible surface and can acknowledge a surface ID, but it never sees
 * a guest WebContents ID, debugger handle, CDP transport, Playwright Browser, or BrowserContext.
 */
export class BrowserSurfaceManager {
  private readonly attachTimeoutMs: number
  private readonly broker: BrowserTargetBroker
  private readonly closeTimeoutMs: number
  private readonly connectOverCdp: (transport: ElectronGuestCdpTransport) => Promise<Browser>
  private readonly closingSurfaceIds = new Set<string>()
  private readonly closeWaiters = new Map<string, PendingClose>()
  private readonly generationBySurface = new Map<string, number>()
  private readonly networkGuard?: BrowserNetworkGuard
  private readonly resolveHost: () => WebContents | null
  private readonly sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
  private readonly surfaces = new Map<string, ManagedSurface>()

  private active?: ActiveAttachment
  private automationEpoch = 0
  private connecting?: Promise<BrowserContext>
  private disposed = false
  private pendingEnsure?: PendingEnsure

  constructor(options: BrowserSurfaceManagerOptions) {
    this.attachTimeoutMs = normalizeTimeout(options.attachTimeoutMs, DEFAULT_ATTACH_TIMEOUT_MS)
    this.broker = options.broker
    this.networkGuard = options.networkGuard
    this.closeTimeoutMs = normalizeTimeout(options.closeTimeoutMs, DEFAULT_CLOSE_TIMEOUT_MS)
    this.resolveHost = options.resolveHost
    this.sendCommand = options.sendCommand
    this.connectOverCdp =
      options.connectOverCdp ??
      ((transport) =>
        chromium.connectOverCDP(transport, {
          isLocal: true,
          noDefaults: true,
          timeout: this.attachTimeoutMs
        }))
  }

  /** ManagedWebviewTargetRegistry entry point used by configureManagedWebviewHost. */
  registerManagedGuest(input: {
    guest: WebContents
    host: WebContents
    partition: string
    surfaceId?: string
  }): void {
    this.assertUsable()
    if (input.partition !== BROWSER_WEBVIEW_PARTITION) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    const urlSurfaceId = parseBrowserSurfaceBootstrapUrl(input.guest.getURL())
    const surfaceId = input.surfaceId ?? urlSurfaceId
    if (!surfaceId) throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    if (urlSurfaceId && urlSurfaceId !== surfaceId) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const previous = this.surfaces.get(surfaceId)
    if (previous && !previous.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (!previous && this.surfaces.size >= MAX_MANAGED_SURFACES) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    this.broker.registerManagedGuest(input)
    this.broker.claimSurface({
      guestWebContentsId: input.guest.id,
      host: input.host,
      surfaceId
    })

    const generation = (this.generationBySurface.get(surfaceId) ?? 0) + 1
    this.generationBySurface.set(surfaceId, generation)
    const handleDestroyed = (): void => this.handleTargetClosed(surfaceId, generation, input.guest)
    const surface: ManagedSurface = {
      generation,
      guest: input.guest,
      handleDestroyed,
      host: input.host,
      surfaceId
    }
    this.surfaces.set(surfaceId, surface)
    input.guest.once('destroyed', handleDestroyed)
    try {
      this.networkGuard?.registerGuest({ generation, guest: input.guest, surfaceId })
    } catch {
      input.guest.removeListener('destroyed', handleDestroyed)
      this.surfaces.delete(surfaceId)
      this.broker.releaseSurface(surfaceId)
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    this.completePendingEnsureIfReady()
  }

  /**
   * Returns the exact BrowserContext backed by the selected right-sidebar webview. Discovery and
   * listTools do not call this method; a managed browser tool does so on first dispatch.
   */
  async getBrowserContext(): Promise<BrowserContext> {
    this.assertUsable()
    if (this.isActiveAttachmentUsable(this.active)) return this.active.context
    if (this.connecting) return this.connecting

    const attempt = this.connectBrowserContext(this.automationEpoch)
    this.connecting = attempt
    try {
      return await attempt
    } finally {
      if (this.connecting === attempt) this.connecting = undefined
    }
  }

  /** Reveals and creates/reuses a page without attaching Playwright. */
  async reveal(): Promise<void> {
    await this.ensureSurface()
  }

  /** Product-semantic alias used by the managed provider before an initial connection. */
  async createOrReusePage(): Promise<void> {
    await this.reveal()
  }

  /**
   * Opens/reuses the exact managed target and serializes one risk-aware tool dispatch on it.
   * Discovery never calls this method.
   */
  async beginNetworkOperation(
    input: BrowserRiskOperationInput
  ): Promise<BrowserNetworkOperationLease> {
    this.assertUsable()
    if (!this.networkGuard) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = await this.ensureSurface()
    if (surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    try {
      return this.networkGuard.beginOperation(surface.guest, input)
    } catch (error) {
      throw new BrowserSurfaceManagerError(
        error instanceof Error && error.message === 'browser.target_closed'
          ? 'browser.target_closed'
          : 'browser.surface_unavailable'
      )
    }
  }

  /** Strict IPC acknowledgement from the trusted Renderer. */
  attach(host: WebContents, input: BrowserSurfaceReadyInput): BrowserSurfaceReadyOutput {
    this.assertUsable()
    const pending = this.pendingEnsure
    if (
      !pending ||
      pending.settled ||
      pending.host !== host ||
      pending.requestId !== input.requestId
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    const surface = this.surfaces.get(input.surfaceId)
    if (surface && surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    pending.readySurfaceId = input.surfaceId
    this.completePendingEnsureIfReady()
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      accepted: true,
      surfaceId: input.surfaceId
    }
  }

  /** Disconnects automation while keeping the user's page and browser partition alive. */
  async detachAutomation(): Promise<void> {
    this.automationEpoch += 1
    this.networkGuard?.deactivateAutomation()
    this.rejectPendingEnsure('browser.surface_unavailable')
    const connecting = this.connecting
    const active = this.active
    this.active = undefined

    active?.transport.close()
    const [connectionSettled, disconnected] = await Promise.all([
      settleWithin(connecting, this.closeTimeoutMs),
      active
        ? boundedWaitFor(() => !active.browser.isConnected(), this.closeTimeoutMs)
        : Promise.resolve(true)
    ])
    if (!connectionSettled || !disconnected) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
  }

  /** Alias used by the managed Playwright host lifecycle. It never closes the manual page. */
  async closeAutomation(): Promise<void> {
    await this.detachAutomation()
  }

  getActiveSurfaceIdentity(): { generation: number; surfaceId: string } | null {
    return this.isActiveAttachmentUsable(this.active)
      ? { generation: this.active.generation, surfaceId: this.active.surfaceId }
      : null
  }

  /** Requests removal of a Browser tab. It never closes the MyCopilot BrowserWindow. */
  async closeSurface(surfaceId?: string): Promise<void> {
    this.assertUsable()
    const targetSurfaceId = surfaceId ?? this.active?.surfaceId
    if (!targetSurfaceId) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    const surface = this.surfaces.get(targetSurfaceId)
    if (!surface) throw new BrowserSurfaceManagerError('browser.target_closed')
    if (this.closeWaiters.has(targetSurfaceId)) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const requestId = randomUUID()
    this.closingSurfaceIds.add(targetSurfaceId)
    const closed = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        const waiter = this.closeWaiters.get(targetSurfaceId)
        if (!waiter || waiter.generation !== surface.generation) return
        this.closeWaiters.delete(targetSurfaceId)
        this.closingSurfaceIds.delete(targetSurfaceId)
        reject(new BrowserSurfaceManagerError('browser.surface_unavailable'))
      }, this.closeTimeoutMs)
      this.closeWaiters.set(targetSurfaceId, {
        generation: surface.generation,
        reject,
        resolve,
        timer
      })
    })
    try {
      this.sendCommand(surface.host, {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'closeSurface',
        requestId,
        surfaceId: targetSurfaceId
      })
      await closed
    } catch (error) {
      const waiter = this.closeWaiters.get(targetSurfaceId)
      if (waiter?.generation === surface.generation) {
        clearTimeout(waiter.timer)
        this.closeWaiters.delete(targetSurfaceId)
      }
      this.closingSurfaceIds.delete(targetSurfaceId)
      throw error
    }
  }

  handleTargetClosed(surfaceId: string, generation: number, guest?: WebContents): void {
    const surface = this.surfaces.get(surfaceId)
    if (
      !surface ||
      surface.generation !== generation ||
      (guest !== undefined && surface.guest !== guest)
    ) {
      return
    }

    surface.guest.removeListener('destroyed', surface.handleDestroyed)
    this.surfaces.delete(surfaceId)
    const closeWaiter = this.closeWaiters.get(surfaceId)
    if (closeWaiter?.generation === generation) {
      clearTimeout(closeWaiter.timer)
      this.closeWaiters.delete(surfaceId)
      closeWaiter.resolve()
    }
    if (this.pendingEnsure?.readySurfaceId === surfaceId) {
      this.rejectPendingEnsure('browser.target_closed')
    }
    if (this.active?.surfaceId === surfaceId && this.active.generation === generation) {
      this.active = undefined
    }
    const wasExplicitlyClosing = this.closingSurfaceIds.delete(surfaceId)
    if (!this.disposed && !wasExplicitlyClosing && !surface.host.isDestroyed()) {
      try {
        this.sendCommand(surface.host, {
          schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
          kind: 'closeSurface',
          requestId: randomUUID(),
          surfaceId
        })
      } catch {
        // Target destruction is already authoritative; UI cleanup is best effort.
      }
    }
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    this.rejectPendingEnsure('browser.manager_shutdown')
    let detachError: unknown
    try {
      await this.detachAutomation()
    } catch (error) {
      detachError = error
    }
    for (const surface of this.surfaces.values()) {
      surface.guest.removeListener('destroyed', surface.handleDestroyed)
    }
    this.surfaces.clear()
    this.closingSurfaceIds.clear()
    for (const waiter of this.closeWaiters.values()) {
      clearTimeout(waiter.timer)
      waiter.reject(new BrowserSurfaceManagerError('browser.manager_shutdown'))
    }
    this.closeWaiters.clear()
    this.broker.dispose()
    await this.networkGuard?.shutdown()
    if (detachError) throw detachError
  }

  snapshot(): {
    attached: boolean
    connecting: boolean
    pendingEnsure: boolean
    surfaces: number
  } {
    return {
      attached: this.isActiveAttachmentUsable(this.active),
      connecting: Boolean(this.connecting),
      pendingEnsure: Boolean(this.pendingEnsure),
      surfaces: this.surfaces.size
    }
  }

  private async connectBrowserContext(expectedEpoch: number): Promise<BrowserContext> {
    const surface = await this.ensureSurface()
    this.assertAttachmentEpoch(expectedEpoch)
    const current = this.surfaces.get(surface.surfaceId)
    if (current !== surface || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    let transport: ElectronGuestCdpTransport | undefined
    let browser: Browser | undefined
    try {
      transport = await this.broker.connect(surface.surfaceId)
      this.assertAttachmentEpoch(expectedEpoch)
      browser = await this.connectOverCdp(transport)
      this.assertAttachmentEpoch(expectedEpoch)
      const contexts = browser.contexts()
      if (contexts.length !== 1 || !contexts[0]) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      if (this.surfaces.get(surface.surfaceId) !== surface || surface.guest.isDestroyed()) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }

      const attachment: ActiveAttachment = {
        browser,
        context: contexts[0],
        generation: surface.generation,
        surfaceId: surface.surfaceId,
        transport
      }
      this.active = attachment
      browser.once('disconnected', () => {
        if (this.active === attachment) this.active = undefined
      })
      return attachment.context
    } catch (error) {
      transport?.close()
      if (error instanceof BrowserSurfaceManagerError) throw error
      throw new BrowserSurfaceManagerError(
        surface.guest.isDestroyed() ? 'browser.target_closed' : 'browser.surface_unavailable'
      )
    }
  }

  private async ensureSurface(): Promise<ManagedSurface> {
    this.assertUsable()
    const activeSurface = this.active ? this.surfaces.get(this.active.surfaceId) : undefined
    if (
      activeSurface &&
      activeSurface.generation === this.active?.generation &&
      !activeSurface.guest.isDestroyed()
    ) {
      return activeSurface
    }
    if (this.pendingEnsure) return this.pendingEnsure.promise

    // A risk preflight intentionally reveals and registers the guest before Playwright attaches.
    // Reuse that exact, Renderer-acknowledged surface instead of issuing a second ensureAttached
    // command. More than one eligible surface is ambiguous and must go back through the trusted UI
    // selection handshake rather than choosing by insertion order.
    const registeredSurface = this.uniqueReusableSurface()
    if (registeredSurface) return registeredSurface

    const host = this.resolveHost()
    if (!host || host.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const requestId = randomUUID()
    let resolve!: (surface: ManagedSurface) => void
    let reject!: (error: BrowserSurfaceManagerError) => void
    const promise = new Promise<ManagedSurface>((promiseResolve, promiseReject) => {
      resolve = promiseResolve
      reject = promiseReject
    })
    const timer = setTimeout(
      () => this.rejectPendingEnsure('browser.surface_unavailable'),
      this.attachTimeoutMs
    )
    this.pendingEnsure = {
      host,
      promise,
      reject,
      requestId,
      resolve,
      settled: false,
      timer
    }

    try {
      this.sendCommand(host, {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        kind: 'ensureAttached',
        requestId
      })
    } catch {
      this.rejectPendingEnsure('browser.surface_unavailable')
    }
    return promise
  }

  private uniqueReusableSurface(): ManagedSurface | undefined {
    const host = this.resolveHost()
    if (!host || host.isDestroyed()) return undefined

    let candidate: ManagedSurface | undefined
    for (const surface of this.surfaces.values()) {
      if (
        surface.host !== host ||
        surface.host.isDestroyed() ||
        surface.guest.isDestroyed() ||
        this.closingSurfaceIds.has(surface.surfaceId)
      ) {
        continue
      }
      if (candidate) return undefined
      candidate = surface
    }
    return candidate
  }

  private completePendingEnsureIfReady(): void {
    const pending = this.pendingEnsure
    if (!pending || pending.settled || !pending.readySurfaceId) return
    const surface = this.surfaces.get(pending.readySurfaceId)
    if (!surface || surface.host !== pending.host || surface.guest.isDestroyed()) return

    pending.settled = true
    clearTimeout(pending.timer)
    this.pendingEnsure = undefined
    pending.resolve(surface)
  }

  private rejectPendingEnsure(code: BrowserSurfaceManagerErrorCode): void {
    const pending = this.pendingEnsure
    if (!pending || pending.settled) return
    pending.settled = true
    clearTimeout(pending.timer)
    this.pendingEnsure = undefined
    pending.reject(new BrowserSurfaceManagerError(code))
  }

  private isActiveAttachmentUsable(
    attachment: ActiveAttachment | undefined
  ): attachment is ActiveAttachment {
    if (!attachment || !attachment.browser.isConnected()) return false
    const surface = this.surfaces.get(attachment.surfaceId)
    return Boolean(
      surface && surface.generation === attachment.generation && !surface.guest.isDestroyed()
    )
  }

  private assertUsable(): void {
    if (this.disposed) throw new BrowserSurfaceManagerError('browser.manager_shutdown')
  }

  private assertAttachmentEpoch(expectedEpoch: number): void {
    this.assertUsable()
    if (expectedEpoch !== this.automationEpoch) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
  }
}

function normalizeTimeout(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, 60_000)
    : fallback
}

async function boundedWaitFor(predicate: () => boolean, timeoutMs: number): Promise<boolean> {
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

async function settleWithin<T>(
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
