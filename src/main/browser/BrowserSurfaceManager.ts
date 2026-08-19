import { randomUUID } from 'node:crypto'
import type { Event, WebContents } from 'electron'
import type { Browser, BrowserContext } from 'playwright'
import { chromium } from 'playwright'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  BROWSER_WEBVIEW_PARTITION,
  parseBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceId,
  type BrowserSurfaceCommand,
  type BrowserSurfaceReadyInput,
  type BrowserSurfaceReadyOutput,
  type BrowserSurfaceSelectedInput,
  type BrowserSurfaceSelectedOutput
} from '@mycopilot/protocol'
import { BrowserTargetBroker } from './BrowserTargetBroker'
import type { ElectronGuestCdpTransport } from './ElectronGuestCdpTransport'
import {
  BrowserNetworkGuard,
  type BrowserMainFrameNavigationFence,
  type BrowserNetworkOperationLease
} from './BrowserNetworkGuard'
import type { BrowserRiskOperationInput } from './BrowserRiskCoordinator'

const DEFAULT_ATTACH_TIMEOUT_MS = 10_000
const DEFAULT_CLOSE_TIMEOUT_MS = 2_000
const DEFAULT_MAX_MANAGED_SURFACES = 8
const MAX_CONFIGURED_MANAGED_SURFACES = 16
const MAX_POPUPS_PER_SECOND = 4
const MAX_PENDING_RENDERER_COMMANDS = 64

export type BrowserSurfaceManagerErrorCode =
  | 'browser.surface_unavailable'
  | 'browser.surface_capacity_exceeded'
  | 'browser.target_closed'
  | 'browser.manager_shutdown'

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
  createSurfaceId?: () => string
  resolveHost: () => WebContents | null
  sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
  maxSurfaces?: number
}

export interface BrowserSurfaceView {
  generation: number
  index: number
  isActive: boolean
  surfaceId: string
  title: string
  url: string
}

/** Main-only document identity. This type must never cross Renderer IPC. */
export interface BrowserSensitiveTargetIdentity {
  generation: number
  navigationEpoch: number
  origin: string
  surfaceId: string
}

export interface BrowserSensitiveDispatchFence {
  finish(): void
}

interface ActiveSensitiveDispatchFence {
  finishSilently(): boolean
}

interface ManagedSurface {
  createdSequence: number
  dispatchFence?: ActiveSensitiveDispatchFence
  generation: number
  guest: WebContents
  handleDidFailLoad: (
    event: Event,
    errorCode: number,
    errorDescription: string,
    validatedURL: string,
    isMainFrame: boolean
  ) => void
  handleDidNavigate: () => void
  handleDidStartNavigation: (
    event: Event,
    url: string,
    isInPlace: boolean,
    isMainFrame: boolean
  ) => void
  handleDidStopLoading: () => void
  handleDestroyed: () => void
  host: WebContents
  navigationEpoch: number
  navigationInProgress: boolean
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
  activate?: boolean
  host: WebContents
  kind: 'ensureAttached' | 'createSurface' | 'selectSurface' | 'resizeSurface'
  dimensions?: { height: number; width: number }
  promise: Promise<ManagedSurface>
  surfaceId: string
  reject: (error: BrowserSurfaceManagerError) => void
  requestId: string
  rendererReady: boolean
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
  private readonly createSurfaceId: () => string
  private readonly closingSurfaceIds = new Set<string>()
  private readonly closeWaiters = new Map<string, PendingClose>()
  private readonly generationBySurface = new Map<string, number>()
  private readonly maxSurfaces: number
  private readonly networkGuard?: BrowserNetworkGuard
  private readonly resolveHost: () => WebContents | null
  private readonly sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
  private readonly surfaces = new Map<string, ManagedSurface>()

  private active?: ActiveAttachment
  private activeSurfaceId?: string
  private automationEpoch = 0
  private connecting?: Promise<BrowserContext>
  private disposed = false
  private readonly pendingSurfaceRequests = new Map<string, PendingEnsure>()
  private createSequence = 0
  private popupWindowStartedAt = 0
  private popupCount = 0
  private needsReveal = false
  private ensuringSurface?: Promise<ManagedSurface>
  private rendererCommandBusy = false
  private readonly rendererCommandQueue: Array<() => void> = []

  constructor(options: BrowserSurfaceManagerOptions) {
    this.attachTimeoutMs = normalizeTimeout(options.attachTimeoutMs, DEFAULT_ATTACH_TIMEOUT_MS)
    this.broker = options.broker
    this.networkGuard = options.networkGuard
    this.closeTimeoutMs = normalizeTimeout(options.closeTimeoutMs, DEFAULT_CLOSE_TIMEOUT_MS)
    this.maxSurfaces = normalizeSurfaceCapacity(options.maxSurfaces)
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
    this.createSurfaceId = options.createSurfaceId ?? (() => `managed-browser-${randomUUID()}`)
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
    if (!previous && this.surfaces.size >= this.maxSurfaces) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }

    this.broker.registerManagedGuest(input)
    const generation = (this.generationBySurface.get(surfaceId) ?? 0) + 1
    this.generationBySurface.set(surfaceId, generation)
    const handleDestroyed = (): void => this.handleTargetClosed(surfaceId, generation, input.guest)
    const handleDidStartNavigation = (
      _event: Event,
      _url: string,
      isInPlace: boolean,
      isMainFrame: boolean
    ): void => {
      const current = this.surfaces.get(surfaceId)
      if (!current || current.generation !== generation || !isMainFrame || isInPlace) return
      current.navigationEpoch += 1
      current.navigationInProgress = true
    }
    const finishMainFrameNavigation = (): void => {
      const current = this.surfaces.get(surfaceId)
      if (current?.generation === generation) current.navigationInProgress = false
    }
    const handleDidFailLoad = (
      _event: Event,
      _errorCode: number,
      _errorDescription: string,
      _validatedURL: string,
      isMainFrame: boolean
    ): void => {
      if (isMainFrame) finishMainFrameNavigation()
    }
    const surface: ManagedSurface = {
      createdSequence: ++this.createSequence,
      generation,
      guest: input.guest,
      handleDidFailLoad,
      handleDidNavigate: finishMainFrameNavigation,
      handleDidStartNavigation,
      handleDidStopLoading: finishMainFrameNavigation,
      handleDestroyed,
      host: input.host,
      navigationEpoch: 1,
      navigationInProgress:
        typeof input.guest.isLoadingMainFrame === 'function' && input.guest.isLoadingMainFrame(),
      surfaceId
    }
    this.surfaces.set(surfaceId, surface)
    input.guest.once('destroyed', handleDestroyed)
    input.guest.on('did-start-navigation', handleDidStartNavigation)
    input.guest.on('did-navigate', surface.handleDidNavigate)
    input.guest.on('did-fail-load', handleDidFailLoad)
    input.guest.on('did-stop-loading', surface.handleDidStopLoading)
    try {
      this.networkGuard?.registerGuest({ generation, guest: input.guest, surfaceId })
    } catch {
      input.guest.removeListener('destroyed', handleDestroyed)
      this.removeNavigationListeners(surface)
      this.surfaces.delete(surfaceId)
      this.broker.releaseSurface(surfaceId)
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    this.completePendingSurfaceRequests()
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
      const lease = this.networkGuard.beginOperation(surface.guest, input)
      await lease.ready?.()
      return lease
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
    const pending = this.pendingSurfaceRequests.get(input.requestId)
    if (
      !pending ||
      pending.settled ||
      pending.host !== host ||
      pending.requestId !== input.requestId ||
      pending.surfaceId !== input.surfaceId
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const resizeMatches =
      pending.kind === 'resizeSurface' &&
      pending.dimensions?.height === input.viewport?.height &&
      pending.dimensions?.width === input.viewport?.width
    if (
      (pending.kind === 'resizeSurface' && !resizeMatches) ||
      (pending.kind !== 'resizeSurface' && input.viewport !== undefined)
    ) {
      this.rejectPendingSurfaceRequest(input.requestId, 'browser.surface_unavailable')
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    const surface = this.surfaces.get(input.surfaceId)
    if (surface && surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    pending.rendererReady = true
    this.completePendingSurfaceRequests()
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      accepted: true,
      surfaceId: input.surfaceId
    }
  }

  selectManualSurface(
    host: WebContents,
    input: BrowserSurfaceSelectedInput
  ): BrowserSurfaceSelectedOutput {
    this.assertUsable()
    const surface = this.surfaces.get(input.surfaceId)
    if (!surface || surface.host !== host || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    // Manual visibility never retargets an in-flight Agent operation. Before automation attaches,
    // it provides the trusted UI selection that ensureActiveSurface should preferentially reuse.
    if (!this.active && !this.connecting) this.activeSurfaceId = surface.surfaceId
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      accepted: true,
      surfaceId: surface.surfaceId
    }
  }

  /** Disconnects automation while keeping the user's page and browser partition alive. */
  async detachAutomation(): Promise<void> {
    this.automationEpoch += 1
    this.needsReveal = true
    this.networkGuard?.deactivateAutomation()
    this.rejectPendingSurfaceRequests('browser.surface_unavailable')
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
    for (const surface of this.surfaces.values()) {
      this.broker.releaseSurface(surface.surfaceId, surface.generation)
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

  /** Returns a stable, non-navigating Main-only document identity without attaching automation. */
  getSensitiveTargetIdentity(): BrowserSensitiveTargetIdentity | null {
    if (this.disposed) return null
    const surfaceId = this.isActiveAttachmentUsable(this.active)
      ? this.active.surfaceId
      : this.activeSurfaceId
    if (!surfaceId) return null
    const surface = this.surfaces.get(surfaceId)
    if (!surface || surface.guest.isDestroyed() || surface.navigationInProgress) return null
    const origin = safeHttpOrigin(surface.guest.getURL())
    if (!origin) return null
    return {
      surfaceId: surface.surfaceId,
      generation: surface.generation,
      navigationEpoch: surface.navigationEpoch,
      origin
    }
  }

  /**
   * Atomically fences the approved document against a main-frame swap for one sensitive dispatch.
   * The permanent navigation epoch rejects work that was already in flight; the network and
   * webContents fences prevent a new redirect/navigation until the original Tool reaches terminal.
   */
  beginSensitiveDispatchFence(
    expected: BrowserSensitiveTargetIdentity
  ): BrowserSensitiveDispatchFence {
    this.assertUsable()
    const initial = this.getSensitiveTargetIdentity()
    if (!sameSensitiveTarget(initial, expected)) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = this.surfaces.get(expected.surfaceId)
    if (
      !surface ||
      surface.generation !== expected.generation ||
      surface.dispatchFence ||
      surface.guest.isDestroyed()
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    let blockedByWebContents = false
    let networkFence: BrowserMainFrameNavigationFence | undefined
    let finished = false
    const blockNavigation = (
      event: Event,
      _url: string,
      _isInPlace: boolean,
      isMainFrame: boolean
    ) => {
      if (!isMainFrame) return
      blockedByWebContents = true
      event.preventDefault()
    }
    surface.guest.on('will-navigate', blockNavigation)
    surface.guest.on('will-redirect', blockNavigation)
    try {
      networkFence = this.networkGuard?.beginMainFrameNavigationFence(
        surface.guest,
        surface.generation
      )
      if (!sameSensitiveTarget(this.getSensitiveTargetIdentity(), expected)) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
    } catch (error) {
      surface.guest.removeListener('will-navigate', blockNavigation)
      surface.guest.removeListener('will-redirect', blockNavigation)
      networkFence?.finish()
      throw error
    }

    const finishSilently = (): boolean => {
      if (finished) return false
      finished = true
      surface.guest.removeListener('will-navigate', blockNavigation)
      surface.guest.removeListener('will-redirect', blockNavigation)
      networkFence?.finish()
      if (surface.dispatchFence === record) surface.dispatchFence = undefined
      return (
        blockedByWebContents ||
        Boolean(networkFence?.blocked()) ||
        !sameSensitiveTarget(this.getSensitiveTargetIdentity(), expected)
      )
    }
    const record: ActiveSensitiveDispatchFence = { finishSilently }
    surface.dispatchFence = record
    return {
      finish: () => {
        if (finishSilently()) {
          throw new BrowserSurfaceManagerError('browser.surface_unavailable')
        }
      }
    }
  }

  ensureGroup(): void {
    this.assertUsable()
    const host = this.resolveHost()
    if (!host || host.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
  }

  async ensureActiveSurface(): Promise<BrowserSurfaceView> {
    return this.toSurfaceView(await this.ensureSurface())
  }

  listSurfaces(): readonly BrowserSurfaceView[] {
    this.assertUsable()
    return this.orderedSurfaces().map((surface, index) => this.toSurfaceView(surface, index))
  }

  async createSurface(
    input: { activate?: boolean; url?: string } = {}
  ): Promise<BrowserSurfaceView> {
    this.assertUsable()
    this.ensureGroup()
    if (this.surfaces.size + this.pendingCreatedSurfaceCount() >= this.maxSurfaces) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }

    const host = this.resolveHost()
    if (!host || host.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surfaceId = this.allocateSurfaceId()
    const shouldActivate = input.activate !== false
    const surface = await this.requestSurface(
      host,
      'createSurface',
      surfaceId,
      undefined,
      shouldActivate
    )
    if (input.url !== undefined) await loadManagedSurface(surface, input.url, this.attachTimeoutMs)
    if (input.activate !== false) await this.switchActiveSurface(surface)
    return this.toSurfaceView(surface)
  }

  async selectSurface(input: { index?: number; surfaceId?: string }): Promise<BrowserSurfaceView> {
    this.assertUsable()
    const surface = this.resolveSurfaceSelection(input)
    const host = this.resolveHost()
    if (!host || host.isDestroyed() || surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    await this.requestSurface(host, 'selectSurface', surface.surfaceId)
    await this.switchActiveSurface(surface)
    return this.toSurfaceView(surface)
  }

  async closeSurfaceByIndex(index?: number): Promise<void> {
    const surface =
      index === undefined
        ? this.activeSurfaceId
          ? this.surfaces.get(this.activeSurfaceId)
          : undefined
        : this.orderedSurfaces()[normalizeSurfaceIndex(index)]
    if (!surface) throw new BrowserSurfaceManagerError('browser.target_closed')
    await this.closeSurface(surface.surfaceId)
  }

  async resizeActiveSurface(input: {
    height: number
    width: number
  }): Promise<{ height: number; width: number }> {
    const identity = this.getActiveSurfaceIdentity()
    if (!identity) throw new BrowserSurfaceManagerError('browser.target_closed')
    const surface = this.surfaces.get(identity.surfaceId)
    const host = this.resolveHost()
    if (!surface || !host || host.isDestroyed() || surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const dimensions = normalizeViewportSize(input)
    await this.requestSurface(host, 'resizeSurface', surface.surfaceId, dimensions)
    return dimensions
  }

  /**
   * Renders the selected managed guest through Electron's native print pipeline.
   *
   * Playwright's `page.pdf()` is not implemented for a BrowserContext connected to an Electron
   * webview target. Keeping this operation on the exact registered guest preserves the Surface
   * Group boundary and returns bytes only to Main; no path or target identity crosses IPC.
   */
  async printActiveSurfaceToPdf(): Promise<Uint8Array> {
    this.assertUsable()
    const identity = this.getActiveSurfaceIdentity()
    if (!identity) throw new BrowserSurfaceManagerError('browser.target_closed')
    const surface = this.surfaces.get(identity.surfaceId)
    if (!surface || surface.generation !== identity.generation || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      const bytes = await Promise.race([
        surface.guest.printToPDF({ printBackground: true }),
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(
            () => reject(new BrowserSurfaceManagerError('browser.surface_unavailable')),
            this.attachTimeoutMs
          )
        })
      ])
      if (surface.guest.isDestroyed()) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }
      if (!(bytes instanceof Uint8Array) || bytes.byteLength === 0) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      return Uint8Array.from(bytes)
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

  /** Requests removal of a Browser tab. It never closes the MyCopilot BrowserWindow. */
  async closeSurface(surfaceId?: string): Promise<void> {
    this.assertUsable()
    const targetSurfaceId = surfaceId ?? this.activeSurfaceId
    if (!targetSurfaceId) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    const surface = this.surfaces.get(targetSurfaceId)
    if (!surface) throw new BrowserSurfaceManagerError('browser.target_closed')
    await this.enqueueRendererCommand(async () => {
      if (this.surfaces.get(targetSurfaceId) !== surface || surface.guest.isDestroyed()) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }
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
    })
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

    const orderedBeforeClose = this.allOrderedSurfaces()
    const closedIndex = orderedBeforeClose.findIndex((candidate) => candidate === surface)
    surface.dispatchFence?.finishSilently()
    surface.guest.removeListener('destroyed', surface.handleDestroyed)
    this.removeNavigationListeners(surface)
    this.surfaces.delete(surfaceId)
    const closeWaiter = this.closeWaiters.get(surfaceId)
    if (closeWaiter?.generation === generation) {
      clearTimeout(closeWaiter.timer)
      this.closeWaiters.delete(surfaceId)
      closeWaiter.resolve()
    }
    this.rejectPendingSurfaceRequestsForSurface(surfaceId, 'browser.target_closed')
    if (this.active?.surfaceId === surfaceId && this.active.generation === generation) {
      this.active = undefined
    }
    if (this.activeSurfaceId === surfaceId) {
      const remaining = orderedBeforeClose.filter((candidate) => candidate !== surface)
      this.activeSurfaceId =
        remaining[Math.min(Math.max(closedIndex, 0), remaining.length - 1)]?.surfaceId
    }
    const wasExplicitlyClosing = this.closingSurfaceIds.delete(surfaceId)
    if (!this.disposed && !wasExplicitlyClosing && !surface.host.isDestroyed()) {
      void this.enqueueRendererCommand(async () => {
        this.sendCommand(surface.host, {
          schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
          kind: 'closeSurface',
          requestId: randomUUID(),
          surfaceId
        })
        // There is no guest left to acknowledge unexpected-target cleanup. Keep the FIFO busy
        // through the next event-loop turn so a later scalar Renderer command cannot replace the
        // close notification in the same React batch.
        await nextEventLoopTurn()
      }).catch(() => undefined)
    }
  }

  /** Creates a denied Electron popup without disrupting the still-running opener tool call. */
  async handlePopup(input: { guest: WebContents; url: string }): Promise<void> {
    this.assertUsable()
    if (input.guest.isDestroyed() || !isSafeManagedPageUrl(input.url)) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const source = [...this.surfaces.values()].find((surface) => surface.guest === input.guest)
    if (!source) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (!this.isActiveAttachmentUsable(this.active)) {
      await loadManagedSurface(source, input.url, this.attachTimeoutMs)
      return
    }

    const now = Date.now()
    if (now - this.popupWindowStartedAt >= 1_000) {
      this.popupWindowStartedAt = now
      this.popupCount = 0
    }
    this.popupCount += 1
    if (this.popupCount > MAX_POPUPS_PER_SECOND) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }
    const popup = await this.createSurface({ activate: false, url: input.url })
    if (popup.isActive) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    this.rejectPendingSurfaceRequests('browser.manager_shutdown')
    let detachError: unknown
    try {
      await this.detachAutomation()
    } catch (error) {
      detachError = error
    }
    for (const surface of this.surfaces.values()) {
      surface.dispatchFence?.finishSilently()
      surface.guest.removeListener('destroyed', surface.handleDestroyed)
      this.removeNavigationListeners(surface)
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
      pendingEnsure: this.pendingSurfaceRequests.size > 0,
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
      this.broker.claimSurface({
        generation: surface.generation,
        guestWebContentsId: surface.guest.id,
        host: surface.host,
        surfaceId: surface.surfaceId
      })
      transport = await this.broker.connect(surface.surfaceId, surface.generation)
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

  private ensureSurface(): Promise<ManagedSurface> {
    if (this.ensuringSurface) return this.ensuringSurface
    const attempt = this.ensureSurfaceOnce()
    this.ensuringSurface = attempt
    void attempt.then(
      () => {
        if (this.ensuringSurface === attempt) this.ensuringSurface = undefined
      },
      () => {
        if (this.ensuringSurface === attempt) this.ensuringSurface = undefined
      }
    )
    return attempt
  }

  private async ensureSurfaceOnce(): Promise<ManagedSurface> {
    this.assertUsable()
    const activeSurface = this.active ? this.surfaces.get(this.active.surfaceId) : undefined
    if (
      activeSurface &&
      activeSurface.generation === this.active?.generation &&
      !activeSurface.guest.isDestroyed()
    ) {
      return activeSurface
    }

    const host = this.resolveHost()
    if (!host || host.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    const selected = this.activeSurfaceId ? this.surfaces.get(this.activeSurfaceId) : undefined
    const reusable =
      selected && !selected.guest.isDestroyed() && !this.closingSurfaceIds.has(selected.surfaceId)
        ? selected
        : this.uniqueReusableSurface()
    if (reusable) {
      if (reusable.surfaceId === this.activeSurfaceId && !this.needsReveal) return reusable
      const revealed = await this.requestSurface(host, 'selectSurface', reusable.surfaceId)
      this.activeSurfaceId = revealed.surfaceId
      this.needsReveal = false
      return revealed
    }

    if (this.surfaces.size + this.pendingCreatedSurfaceCount() >= this.maxSurfaces) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }
    const surfaceId = this.allocateSurfaceId()
    const created = await this.requestSurface(host, 'ensureAttached', surfaceId)
    this.activeSurfaceId = created.surfaceId
    this.needsReveal = false
    return created
  }

  private requestSurface(
    host: WebContents,
    kind: PendingEnsure['kind'],
    surfaceId: string,
    dimensions?: { height: number; width: number },
    activate = true
  ): Promise<ManagedSurface> {
    return this.enqueueRendererCommand(() =>
      this.dispatchSurfaceRequest(host, kind, surfaceId, dimensions, activate)
    )
  }

  private enqueueRendererCommand<T>(operation: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (
        this.rendererCommandBusy &&
        this.rendererCommandQueue.length >= MAX_PENDING_RENDERER_COMMANDS
      ) {
        reject(new BrowserSurfaceManagerError('browser.surface_unavailable'))
        return
      }
      const run = (): void => {
        let dispatched: Promise<T>
        try {
          this.assertUsable()
          dispatched = operation()
        } catch (error) {
          this.finishRendererCommand()
          reject(error)
          return
        }
        void dispatched.then(resolve, reject).finally(() => this.finishRendererCommand())
      }
      if (this.rendererCommandBusy) this.rendererCommandQueue.push(run)
      else {
        this.rendererCommandBusy = true
        run()
      }
    })
  }

  private finishRendererCommand(): void {
    const next = this.rendererCommandQueue.shift()
    if (next) {
      next()
      return
    }
    this.rendererCommandBusy = false
  }

  private dispatchSurfaceRequest(
    host: WebContents,
    kind: PendingEnsure['kind'],
    surfaceId: string,
    dimensions?: { height: number; width: number },
    activate = true
  ): Promise<ManagedSurface> {
    if (
      (kind === 'createSurface' || kind === 'ensureAttached') &&
      !this.surfaces.has(surfaceId) &&
      this.surfaces.size >= this.maxSurfaces
    ) {
      return Promise.reject(new BrowserSurfaceManagerError('browser.surface_capacity_exceeded'))
    }
    const duplicate = [...this.pendingSurfaceRequests.values()].find(
      (pending) =>
        !pending.settled &&
        pending.host === host &&
        pending.kind === kind &&
        pending.surfaceId === surfaceId &&
        (kind !== 'resizeSurface' ||
          (pending.dimensions?.height === dimensions?.height &&
            pending.dimensions?.width === dimensions?.width))
    )
    if (duplicate) return duplicate.promise

    const requestId = randomUUID()
    let resolve!: (surface: ManagedSurface) => void
    let reject!: (error: BrowserSurfaceManagerError) => void
    const promise = new Promise<ManagedSurface>((promiseResolve, promiseReject) => {
      resolve = promiseResolve
      reject = promiseReject
    })
    const timer = setTimeout(() => {
      this.rejectPendingSurfaceRequest(requestId, 'browser.surface_unavailable')
    }, this.attachTimeoutMs)
    const pending: PendingEnsure = {
      ...(kind === 'createSurface' ? { activate } : {}),
      ...(dimensions ? { dimensions } : {}),
      host,
      kind,
      promise,
      reject,
      requestId,
      rendererReady: false,
      resolve,
      settled: false,
      surfaceId,
      timer
    }
    this.pendingSurfaceRequests.set(requestId, pending)

    try {
      this.sendCommand(
        host,
        kind === 'resizeSurface'
          ? {
              schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
              kind,
              requestId,
              surfaceId,
              ...(dimensions ?? normalizeViewportSize({ height: 720, width: 1_280 }))
            }
          : kind === 'createSurface'
            ? {
                schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
                kind,
                requestId,
                surfaceId,
                activate
              }
            : {
                schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
                kind,
                requestId,
                surfaceId
              }
      )
    } catch {
      this.rejectPendingSurfaceRequest(requestId, 'browser.surface_unavailable')
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

  private completePendingSurfaceRequests(): void {
    for (const pending of this.pendingSurfaceRequests.values()) {
      if (pending.settled || !pending.rendererReady) continue
      const surface = this.surfaces.get(pending.surfaceId)
      if (!surface || surface.host !== pending.host || surface.guest.isDestroyed()) continue

      pending.settled = true
      clearTimeout(pending.timer)
      this.pendingSurfaceRequests.delete(pending.requestId)
      pending.resolve(surface)
    }
  }

  private rejectPendingSurfaceRequest(
    requestId: string,
    code: BrowserSurfaceManagerErrorCode
  ): void {
    const pending = this.pendingSurfaceRequests.get(requestId)
    if (!pending || pending.settled) return
    pending.settled = true
    clearTimeout(pending.timer)
    this.pendingSurfaceRequests.delete(requestId)
    pending.reject(new BrowserSurfaceManagerError(code))
  }

  private rejectPendingSurfaceRequests(code: BrowserSurfaceManagerErrorCode): void {
    for (const requestId of [...this.pendingSurfaceRequests.keys()]) {
      this.rejectPendingSurfaceRequest(requestId, code)
    }
  }

  private rejectPendingSurfaceRequestsForSurface(
    surfaceId: string,
    code: BrowserSurfaceManagerErrorCode
  ): void {
    for (const pending of [...this.pendingSurfaceRequests.values()]) {
      if (pending.surfaceId === surfaceId) {
        this.rejectPendingSurfaceRequest(pending.requestId, code)
      }
    }
  }

  private pendingCreatedSurfaceCount(): number {
    return [...this.pendingSurfaceRequests.values()].filter(
      (pending) => pending.kind === 'createSurface' || pending.kind === 'ensureAttached'
    ).length
  }

  private orderedSurfaces(): ManagedSurface[] {
    return this.allOrderedSurfaces().filter(
      (surface) => !surface.guest.isDestroyed() && !this.closingSurfaceIds.has(surface.surfaceId)
    )
  }

  private allOrderedSurfaces(): ManagedSurface[] {
    return [...this.surfaces.values()].sort(
      (left, right) => left.createdSequence - right.createdSequence
    )
  }

  private allocateSurfaceId(): string {
    for (let attempt = 0; attempt < 16; attempt += 1) {
      const candidate = parseBrowserSurfaceId(this.createSurfaceId())
      if (
        !this.surfaces.has(candidate) &&
        ![...this.pendingSurfaceRequests.values()].some(
          (pending) => pending.surfaceId === candidate
        )
      ) {
        return candidate
      }
    }
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }

  private resolveSurfaceSelection(input: { index?: number; surfaceId?: string }): ManagedSurface {
    if (input.surfaceId !== undefined) {
      const surface = this.surfaces.get(input.surfaceId)
      if (surface && !surface.guest.isDestroyed()) return surface
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    if (input.index === undefined) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = this.orderedSurfaces()[normalizeSurfaceIndex(input.index)]
    if (!surface) throw new BrowserSurfaceManagerError('browser.target_closed')
    return surface
  }

  private toSurfaceView(surface: ManagedSurface, knownIndex?: number): BrowserSurfaceView {
    return {
      generation: surface.generation,
      index: knownIndex ?? this.orderedSurfaces().indexOf(surface),
      isActive: surface.surfaceId === this.activeSurfaceId,
      surfaceId: surface.surfaceId,
      title: safeSurfaceTitle(surface.guest.getTitle()),
      url: safeSurfaceUrl(surface.guest.getURL())
    }
  }

  private async switchActiveSurface(surface: ManagedSurface): Promise<void> {
    const active = this.active
    if (
      active &&
      (active.surfaceId !== surface.surfaceId || active.generation !== surface.generation)
    ) {
      this.automationEpoch += 1
      this.active = undefined
      active.transport.close()
      const disconnected = await boundedWaitFor(
        () => !active.browser.isConnected(),
        this.closeTimeoutMs
      )
      if (!disconnected) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
    }
    this.activeSurfaceId = surface.surfaceId
    this.needsReveal = false
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

  private removeNavigationListeners(surface: ManagedSurface): void {
    surface.guest.removeListener('did-start-navigation', surface.handleDidStartNavigation)
    surface.guest.removeListener('did-navigate', surface.handleDidNavigate)
    surface.guest.removeListener('did-fail-load', surface.handleDidFailLoad)
    surface.guest.removeListener('did-stop-loading', surface.handleDidStopLoading)
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

/** Round-2 product name; the compatibility export keeps existing Main wiring source-stable. */
export { BrowserSurfaceManager as BrowserSurfaceGroupManager }

function normalizeTimeout(value: number | undefined, fallback: number): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, 60_000)
    : fallback
}

function normalizeSurfaceCapacity(value: number | undefined): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, MAX_CONFIGURED_MANAGED_SURFACES)
    : DEFAULT_MAX_MANAGED_SURFACES
}

function normalizeSurfaceIndex(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }
  return value
}

function normalizeViewportSize(input: { height: number; width: number }): {
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

function safeSurfaceTitle(value: string): string {
  const normalized = value.replace(/\p{Cc}/gu, ' ').trim()
  return normalized.slice(0, 256) || 'New tab'
}

function safeSurfaceUrl(value: string): string {
  try {
    const parsed = new URL(value)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return 'about:blank'
    parsed.username = ''
    parsed.password = ''
    parsed.search = ''
    parsed.hash = ''
    return parsed.toString().slice(0, 2_048)
  } catch {
    return 'about:blank'
  }
}

function safeHttpOrigin(value: string): string | null {
  try {
    const parsed = new URL(value)
    if (
      !['http:', 'https:'].includes(parsed.protocol) ||
      parsed.username !== '' ||
      parsed.password !== ''
    ) {
      return null
    }
    return parsed.origin
  } catch {
    return null
  }
}

function sameSensitiveTarget(
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

function isSafeManagedPageUrl(value: string): boolean {
  try {
    const protocol = new URL(value).protocol
    return protocol === 'http:' || protocol === 'https:'
  } catch {
    return false
  }
}

async function loadManagedSurface(
  surface: ManagedSurface,
  url: string,
  timeoutMs: number
): Promise<void> {
  if (!isSafeManagedPageUrl(url) || surface.guest.isDestroyed()) {
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

async function nextEventLoopTurn(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve))
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
