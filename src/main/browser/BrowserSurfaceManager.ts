import { randomUUID } from 'node:crypto'
import type {
  BrowserWindow,
  BrowserWindowConstructorOptions,
  Event,
  Input,
  RenderProcessGoneDetails,
  WebContents
} from 'electron'
import type { Browser, BrowserContext, ConnectOverCDPTransport } from 'playwright'
import { chromium } from 'playwright'
import {
  BROWSER_SURFACE_SCHEMA_VERSION,
  BROWSER_WEBVIEW_PARTITION,
  parseBrowserSurfaceBootstrapUrl,
  parseBrowserSurfaceId,
  type BrowserSurfaceActionInput,
  type BrowserSurfaceCommand,
  type BrowserSurfacePublicCrashError,
  type BrowserSurfacePublicLoadError,
  type BrowserSurfaceReadyInput,
  type BrowserSurfaceReadyOutput,
  type BrowserSurfaceSelectedInput,
  type BrowserSurfaceSelectedOutput,
  type BrowserSurfaceState,
  type BrowserSurfaceStateInput
} from '@mycopilot/protocol'
import { BrowserTargetBroker } from './BrowserTargetBroker'
import { printManagedGuestToPdf } from './ElectronGuestPdfPrinter'
import { observeNativePopupLoad } from './BrowserPopupLifecycle'
import type { ManagedTargetCreationIntent } from './ElectronSurfaceGroupCdpTransport'
import type { ElectronSurfaceGroupCdpTransport } from './ElectronSurfaceGroupCdpTransport'
import {
  BrowserNetworkGuard,
  type BrowserMainFrameNavigationFence,
  type BrowserNetworkOperationLease,
  type BrowserTargetCreationAuthority
} from './BrowserNetworkGuard'
import type { BrowserRiskOperationInput } from './BrowserRiskCoordinator'
import { BrowserInternalPageLifecycle } from './BrowserInternalPageLifecycle'
import type { BrowserInternalPageStoreLike } from './BrowserInternalPageStore'
import {
  createBrowserSurfaceLoadError,
  createBrowserSurfaceCrashError,
  isBrowserInternalActionUrl,
  toPublicBrowserSurfaceCrashError,
  toPublicBrowserSurfaceLoadError,
  type BrowserSurfaceCrashError,
  type BrowserSurfaceLoadError
} from './BrowserLoadErrorPage'
import {
  boundedWaitFor,
  browserNavigationErrorDescription,
  DEFAULT_ATTACH_TIMEOUT_MS,
  DEFAULT_CLOSE_TIMEOUT_MS,
  fallbackBrowserSurfaceCrashError,
  fallbackSurfaceTitle,
  haveSameHttpOrigin,
  INTERNAL_ERROR_PAGE_MAX_ATTEMPTS,
  INTERNAL_ERROR_PAGE_RETRY_DELAY_MS,
  isIgnoredBrowserLoadFailure,
  isManagedBlankSurfaceUrl,
  isNavigationAlreadyPendingError,
  isSafeManagedPageUrl,
  loadManagedBlankSurface,
  loadManagedSurface,
  MAX_PENDING_RENDERER_COMMANDS,
  MAX_POPUPS_PER_SECOND,
  MAX_SETTLED_SURFACE_REQUESTS,
  nextEventLoopTurn,
  normalizeRendererGoneReason,
  normalizeSurfaceCapacity,
  normalizeSurfaceIndex,
  normalizeTimeout,
  normalizeViewportSize,
  runGuestHistoryAction,
  safeGuestHistoryBoolean,
  safeHttpOrigin,
  safeLogicalSurfaceUrl,
  safeRemoteResourceUrl,
  safeSurfaceTitle,
  safeSurfaceUrl,
  sameSensitiveTarget,
  settleWithin,
  STRICT_MODE_SURFACE_HANDOFF_MS,
  waitForInitialDocumentReady
} from './BrowserSurfaceHelpers'
import {
  BrowserSurfaceManagerError,
  type ActiveAttachment,
  type ActiveSensitiveDispatchFence,
  type ActiveTargetCreationIntent,
  type BrowserInternalDocument,
  type BrowserSensitiveDispatchFence,
  type BrowserSensitiveTargetIdentity,
  type BrowserSurfaceDiagnostic,
  type BrowserSurfaceHistoryEvent,
  type BrowserSurfaceManagerOptions,
  type BrowserSurfaceManagerErrorCode,
  type BrowserSurfaceView,
  type BrowserToolSurfaceLease,
  type InternalPageLoad,
  type ManagedSurface,
  type PendingClose,
  type PendingEnsure,
  type PendingSurfaceGroupAdmission,
  type PendingSurfaceHandoff,
  type SettledSurfaceReadyReason,
  type SettledSurfaceRequest
} from './BrowserSurfaceTypes'

export { BrowserSurfaceManagerError } from './BrowserSurfaceTypes'
export type {
  BrowserSensitiveDispatchFence,
  BrowserSensitiveTargetIdentity,
  BrowserSurfaceDiagnostic,
  BrowserSurfaceHistoryEvent,
  BrowserSurfaceManagerErrorCode,
  BrowserSurfaceManagerOptions,
  BrowserSurfaceView,
  BrowserToolSurfaceLease
} from './BrowserSurfaceTypes'

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
  private readonly connectOverCdp: (transport: ConnectOverCDPTransport) => Promise<Browser>
  private readonly createSurfaceId: () => string
  private readonly getLocale: () => string
  private readonly internalPages: BrowserInternalPageLifecycle
  private readonly internalPageStore: BrowserInternalPageStoreLike
  private readonly onHistoryMetadata?: BrowserSurfaceManagerOptions['onHistoryMetadata']
  private readonly onHistoryNavigation?: BrowserSurfaceManagerOptions['onHistoryNavigation']
  private readonly closingSurfaceIds = new Set<string>()
  private readonly closeWaiters = new Map<string, PendingClose>()
  private readonly generationBySurface = new Map<string, number>()
  private readonly maxSurfaces: number
  private readonly networkGuard?: BrowserNetworkGuard
  private readonly recordDiagnostic: (diagnostic: BrowserSurfaceDiagnostic) => void
  private readonly releaseSurfaceResources?: BrowserSurfaceManagerOptions['releaseSurfaceResources']
  private readonly resolveHost: () => WebContents | null
  private readonly sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
  private readonly sendState: (host: WebContents, state: BrowserSurfaceState) => void
  private readonly surfaces = new Map<string, ManagedSurface>()
  private readonly surfaceGroupAdmissions = new Map<string, PendingSurfaceGroupAdmission>()
  private readonly suppressAutomaticGroupAdmission = new Set<string>()
  private readonly suppressDeferredGroupAdmissionReservation = new Set<string>()
  private groupAdmissionTail: Promise<void> = Promise.resolve()

  private active?: ActiveAttachment
  private automationSurfaceId?: string
  private activeSurfaceId?: string
  private automationEpoch = 0
  private connecting?: Promise<BrowserContext>
  private disposed = false
  private readonly pendingSurfaceRequests = new Map<string, PendingEnsure>()
  private readonly settledSurfaceRequests = new Map<string, SettledSurfaceRequest>()
  private readonly pendingSurfaceHandoffs = new Map<string, PendingSurfaceHandoff>()
  private createSequence = 0
  private popupWindowStartedAt = 0
  private popupCount = 0
  private needsReveal = false
  private ensuringSurface?: Promise<ManagedSurface>
  private rendererCommandBusy = false
  private readonly rendererCommandQueue: Array<() => void> = []
  private targetCreation?: ActiveTargetCreationIntent
  /** Highest bound Renderer intent observed; probes and rejected identities do not advance it. */
  private rendererSelectionRevision = 0
  /** Main-owned mutation epoch retained by exact in-flight Tool leases. */
  private manualSelectionRevision = 0
  private toolSurfaceLease?: {
    generation: number
    selectionRevision: number
    surfaceId: string
  }

  constructor(options: BrowserSurfaceManagerOptions) {
    this.attachTimeoutMs = normalizeTimeout(options.attachTimeoutMs, DEFAULT_ATTACH_TIMEOUT_MS)
    this.broker = options.broker
    this.networkGuard = options.networkGuard
    this.recordDiagnostic =
      options.recordDiagnostic ??
      ((diagnostic) => {
        console.warn('Managed browser renderer recovery event', diagnostic)
      })
    this.releaseSurfaceResources = options.releaseSurfaceResources
    this.closeTimeoutMs = normalizeTimeout(options.closeTimeoutMs, DEFAULT_CLOSE_TIMEOUT_MS)
    this.maxSurfaces = normalizeSurfaceCapacity(options.maxSurfaces)
    this.resolveHost = options.resolveHost
    this.sendCommand = options.sendCommand
    this.sendState = options.sendState ?? (() => undefined)
    this.getLocale = options.getLocale ?? (() => 'zh-CN')
    this.internalPageStore = options.internalPageStore
    this.internalPages = new BrowserInternalPageLifecycle({
      isManagedSurfaceCurrent: (surface) =>
        !this.disposed && this.surfaces.get(surface.surfaceId) === surface,
      networkGuard: this.networkGuard,
      publishSurfaceState: (surface) => this.publishSurfaceState(surface),
      store: this.internalPageStore
    })
    this.onHistoryMetadata = options.onHistoryMetadata
    this.onHistoryNavigation = options.onHistoryNavigation
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
    documentReady?: boolean
    guest: WebContents
    host: WebContents
    partition: string
    surfaceId?: string
    nativePopup?: { window: BrowserWindow; openerGuest: WebContents }
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

    const opener = input.nativePopup
      ? [...this.surfaces.values()].find(
          (surface) => surface.guest === input.nativePopup!.openerGuest
        )
      : undefined
    if (input.nativePopup) {
      if (
        !opener ||
        input.nativePopup.window.isDestroyed() ||
        input.nativePopup.window.webContents !== input.guest
      ) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      this.broker.registerManagedPopup({ ...input, openerGuest: input.nativePopup.openerGuest })
    } else {
      this.broker.registerManagedGuest(input)
    }
    const generation = (this.generationBySurface.get(surfaceId) ?? 0) + 1
    this.generationBySurface.set(surfaceId, generation)
    try {
      this.broker.claimSurface({
        generation,
        guestWebContentsId: input.guest.id,
        host: input.host,
        surfaceId
      })
    } catch {
      this.broker.unregisterManagedGuest({ guest: input.guest, host: input.host })
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const handleDestroyed = (): void => this.handleTargetClosed(surfaceId, generation, input.guest)
    let resolveInitialDocumentReady = (): void => undefined
    const initialDocumentReady = input.documentReady !== false
    const initialDocumentReadyPromise = initialDocumentReady
      ? Promise.resolve()
      : new Promise<void>((resolve) => {
          resolveInitialDocumentReady = resolve
        })
    const handleDidStartNavigation = (
      event: Event,
      url: string,
      isInPlace: boolean,
      isMainFrame: boolean
    ): void =>
      this.handleSurfaceDidStartNavigation({
        event,
        generation,
        guest: input.guest,
        isInPlace,
        isMainFrame,
        surfaceId,
        url
      })
    const handleDidFailLoad = (
      _event: Event,
      errorCode: number,
      errorDescription: string,
      validatedURL: string,
      isMainFrame: boolean
    ): void =>
      this.handleSurfaceDidFailLoad({
        errorCode,
        errorDescription,
        generation,
        guest: input.guest,
        isMainFrame,
        surfaceId,
        validatedURL
      })
    const handleDidNavigate = (_event: Event, url: string): void =>
      this.handleSurfaceDidNavigate(surfaceId, generation, input.guest, url)
    const handleDidRedirectNavigation = (
      _event: Event,
      url: string,
      isInPlace: boolean,
      isMainFrame: boolean
    ): void => {
      if (!isInPlace && isMainFrame) {
        this.handleSurfaceDidRedirectNavigation(surfaceId, generation, input.guest, url)
      }
    }
    const handleDidNavigateInPage = (_event: Event, url: string, isMainFrame: boolean): void => {
      if (isMainFrame) this.handleSurfaceDidNavigateInPage(surfaceId, generation, input.guest, url)
    }
    const handleDidStopLoading = (): void =>
      this.handleSurfaceDidStopLoading(surfaceId, generation, input.guest)
    const handleTitleUpdated = (_event: Event, title: string): void =>
      this.handleSurfaceTitleUpdated(surfaceId, generation, input.guest, title)
    const handleFaviconUpdated = (_event: Event, favicons: string[]): void =>
      this.handleSurfaceFaviconUpdated(surfaceId, generation, input.guest, favicons)
    const handleBeforeInputEvent = (event: Event, keyboardInput: Input): void =>
      this.handleSurfaceBeforeInputEvent(surfaceId, generation, input.guest, event, keyboardInput)
    const handleRenderProcessGone = (_event: Event, details: RenderProcessGoneDetails): void =>
      this.handleSurfaceRenderProcessGone(surfaceId, generation, input.guest, details)
    const handleUnresponsive = (): void =>
      this.handleSurfaceUnresponsive(surfaceId, generation, input.guest)
    const handleResponsive = (): void => {
      // Recovery remains user-driven. A responsive signal alone never reloads or clears the page.
    }
    const handleInitialDocumentReady = (): void => {
      const current = this.surfaces.get(surfaceId)
      if (
        !current ||
        current.generation !== generation ||
        current.guest !== input.guest ||
        current.initialDocumentReady
      ) {
        return
      }
      current.initialDocumentReady = true
      if (current.initialDocumentReadyTimer) clearTimeout(current.initialDocumentReadyTimer)
      current.initialDocumentReadyTimer = undefined
      resolveInitialDocumentReady()
      input.guest.removeListener('dom-ready', handleInitialDocumentReady)
      input.guest.removeListener('did-finish-load', handleInitialDocumentReady)
      this.completePendingSurfaceRequests()
    }
    const initialLogicalUrl = safeLogicalSurfaceUrl(input.guest.getURL())
    const surface: ManagedSurface = {
      ...(input.nativePopup && opener
        ? {
            nativePopup: {
              window: input.nativePopup.window,
              openerSurfaceId: opener.surfaceId,
              openerGeneration: opener.generation,
              cleanup: () => undefined
            }
          }
        : {}),
      createdSequence: ++this.createSequence,
      generation,
      guest: input.guest,
      handleBeforeInputEvent,
      handleDidFailLoad,
      handleDidNavigate,
      handleDidNavigateInPage,
      handleDidRedirectNavigation,
      handleDidStartNavigation,
      handleDidStopLoading,
      handleDestroyed,
      handleFaviconUpdated,
      handleRenderProcessGone,
      handleResponsive,
      handleUnresponsive,
      handleInitialDocumentReady,
      handleTitleUpdated,
      host: input.host,
      initialDocumentReady,
      initialDocumentReadyPromise,
      internalDocuments: new Map(),
      logicalFaviconUrl: null,
      logicalTitle: initialLogicalUrl ? safeSurfaceTitle(input.guest.getTitle()) : null,
      logicalUrl: initialLogicalUrl,
      navigationEpoch: 1,
      navigationInProgress:
        typeof input.guest.isLoadingMainFrame === 'function' && input.guest.isLoadingMainFrame(),
      navigationUrls: new Set(initialLogicalUrl ? [initialLogicalUrl] : []),
      pendingHistoryRemovals: [],
      pendingNavigationUrl: initialLogicalUrl,
      preparedNavigationUrl: null,
      presentation: 'content',
      releaseInitialDocumentReady: resolveInitialDocumentReady,
      surfaceInstanceId: randomUUID(),
      surfaceId,
      stateRevision: 0
    }
    this.surfaces.set(surfaceId, surface)
    this.broker.setSurfacePresentationResolver(surfaceId, generation, (physicalUrl) => {
      const current = this.surfaces.get(surfaceId)
      if (current !== surface || current.generation !== generation) return null
      const document = current.internalDocuments.get(physicalUrl)
      if (!document && physicalUrl !== current.guest.getURL()) return null
      const logicalUrl = document?.logicalUrl ?? current.logicalUrl
      if (!logicalUrl) return null
      return {
        title:
          document?.loadError?.title ??
          document?.crashError?.title ??
          current.logicalTitle ??
          fallbackSurfaceTitle(logicalUrl),
        url: logicalUrl
      }
    })
    input.guest.once('destroyed', handleDestroyed)
    input.guest.on('before-input-event', handleBeforeInputEvent)
    input.guest.on('did-start-navigation', handleDidStartNavigation)
    input.guest.on('did-redirect-navigation', handleDidRedirectNavigation)
    input.guest.on('did-navigate', surface.handleDidNavigate)
    input.guest.on('did-navigate-in-page', surface.handleDidNavigateInPage)
    input.guest.on('did-fail-load', handleDidFailLoad)
    input.guest.on('did-stop-loading', surface.handleDidStopLoading)
    input.guest.on('page-title-updated', surface.handleTitleUpdated)
    input.guest.on('page-favicon-updated', surface.handleFaviconUpdated)
    input.guest.on('render-process-gone', handleRenderProcessGone)
    input.guest.on('responsive', handleResponsive)
    input.guest.on('unresponsive', handleUnresponsive)
    if (!surface.initialDocumentReady) {
      input.guest.on('dom-ready', handleInitialDocumentReady)
      input.guest.on('did-finish-load', handleInitialDocumentReady)
      surface.initialDocumentReadyTimer = setTimeout(() => {
        if (
          this.surfaces.get(surfaceId) === surface &&
          !surface.initialDocumentReady &&
          !input.guest.isDestroyed()
        ) {
          input.guest.close()
        }
      }, this.attachTimeoutMs)
    }
    try {
      this.networkGuard?.registerGuest({
        generation,
        guest: input.guest,
        surfaceId,
        ...(input.nativePopup ? { trustedNativePopup: true } : {})
      })
    } catch {
      input.guest.removeListener('destroyed', handleDestroyed)
      this.removeNavigationListeners(surface)
      this.surfaces.delete(surfaceId)
      this.broker.unregisterManagedGuest({ guest: input.guest, host: input.host })
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    this.cancelPendingSurfaceHandoff(surfaceId, input.host)
    // Electron can publish did-attach-webview after the inert about:blank bootstrap has already
    // crossed dom-ready. The listeners above close the ordinary race; this post-registration
    // state check closes the missed-event edge without allowing completion before NetworkGuard
    // has accepted this exact generation. Renderer readiness is independently gated on its exact
    // webview's document-ready event.
    if (
      !surface.initialDocumentReady &&
      !input.guest.isDestroyed() &&
      typeof input.guest.isLoadingMainFrame === 'function' &&
      !input.guest.isLoadingMainFrame()
    ) {
      handleInitialDocumentReady()
    } else {
      this.completePendingSurfaceRequests()
    }
    if (this.isActiveAttachmentUsable(this.active)) {
      if (this.suppressAutomaticGroupAdmission.has(surfaceId)) {
        // Target.createTarget must finish loading its Host-owned URL before Playwright sees the
        // page, but it still reserves its createdSequence position immediately. Later manual or
        // popup tabs therefore cannot overtake it in Context._tabs.
        if (!this.suppressDeferredGroupAdmissionReservation.has(surfaceId)) {
          this.reserveSurfaceGroupAdmission(surface, true)
        }
      } else {
        const attachment = this.active
        void this.addSurfaceToActiveGroup(surface).catch(() => {
          // Detach/reconnect invalidates an admission attempt without invalidating the user's
          // healthy manual page. Only retire a surface for a failure on the same live attachment.
          if (this.active === attachment && this.isActiveAttachmentUsable(attachment)) {
            this.handleTargetClosed(surface.surfaceId, surface.generation, surface.guest)
          }
        })
      }
    }
  }

  /** ManagedWebviewTargetRegistry exact retained-document check. */
  isInternalNavigationAllowed(guest: WebContents, url: string): boolean {
    const surface = [...this.surfaces.values()].find((candidate) => candidate.guest === guest)
    return Boolean(
      surface &&
      surface.internalDocuments.get(url)?.generation === surface.generation &&
      this.networkGuard?.isInternalNavigationAllowed(guest, url) !== false
    )
  }

  /** Consumes only the opaque title signal owned by the exact active Main document. */
  private handleInternalPageAction(surface: ManagedSurface, action: string): boolean {
    if (!isBrowserInternalActionUrl(action)) return false
    if (surface.guest.isDestroyed()) return false
    const document = surface.internalDocuments.get(surface.guest.getURL())
    if (
      !document ||
      document.generation !== surface.generation ||
      document.actionUrl !== action ||
      document.internalPageUrl !== surface.guest.getURL()
    ) {
      return false
    }
    setImmediate(() => {
      if (this.surfaces.get(surface.surfaceId) !== surface || surface.guest.isDestroyed()) return
      this.retrySurface(surface)
    })
    return true
  }

  getSurfaceState(host: WebContents, input: BrowserSurfaceStateInput): BrowserSurfaceState {
    this.assertUsable()
    return this.toRendererSurfaceState(this.resolveExactRendererSurface(host, input))
  }

  performSurfaceAction(host: WebContents, input: BrowserSurfaceActionInput): BrowserSurfaceState {
    this.assertUsable()
    const surface = this.resolveExactRendererSurface(host, input)
    if (input.action === 'navigate') {
      if (!input.url) throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      this.loadRendererRequestedUrl(surface, input.url, {
        replaceInternalEntry: surface.loadError?.failedUrl === input.url
      })
    } else if (input.action === 'reload') {
      this.retrySurface(surface)
    } else if (input.action === 'goBack') {
      if (safeGuestHistoryBoolean(surface.guest, 'canGoBack')) {
        this.runGuestNavigationCommand(surface, () =>
          runGuestHistoryAction(surface.guest, 'goBack')
        )
      }
    } else if (safeGuestHistoryBoolean(surface.guest, 'canGoForward')) {
      this.runGuestNavigationCommand(surface, () =>
        runGuestHistoryAction(surface.guest, 'goForward')
      )
    }
    return this.toRendererSurfaceState(surface)
  }

  /**
   * Returns the exact BrowserContext backed by the selected right-sidebar webview. Discovery and
   * listTools do not call this method; a managed browser tool does so on first dispatch.
   */
  async getBrowserContext(options: { createVisiblePage?: boolean } = {}): Promise<BrowserContext> {
    this.assertUsable()
    if (this.isActiveAttachmentUsable(this.active)) return this.active.context
    if (this.connecting) return this.connecting

    const attempt = this.connectBrowserContext(
      this.automationEpoch,
      options.createVisiblePage !== false
    )
    this.connecting = attempt
    try {
      return await attempt
    } finally {
      if (this.connecting === attempt) this.connecting = undefined
    }
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
    const leased = this.toolSurfaceLease
    const surface = leased ? this.surfaces.get(leased.surfaceId) : await this.ensureSurface()
    if (!surface || (leased && surface.generation !== leased.generation)) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
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
    if (!pending) return this.settledSurfaceReadyOutput(host, input)
    if (
      pending.settled ||
      pending.host !== host ||
      pending.requestId !== input.requestId ||
      pending.surfaceId !== input.surfaceId
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (!this.readyViewportMatches(pending, input)) {
      this.rejectPendingSurfaceRequest(
        input.requestId,
        'browser.surface_unavailable',
        'request_cancelled'
      )
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }

    const surface = this.surfaces.get(input.surfaceId)
    if (!surface || surface.guest.isDestroyed()) {
      return {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        accepted: false,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        requestId: input.requestId,
        ...(input.surfaceInstanceId ? { surfaceInstanceId: input.surfaceInstanceId } : {}),
        surfaceId: input.surfaceId
      }
    }
    if (surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (input.surfaceInstanceId !== surface.surfaceInstanceId) {
      return {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        accepted: false,
        status: 'stale',
        reason: 'instance_mismatch',
        retryable: true,
        requestId: input.requestId,
        surfaceInstanceId: surface.surfaceInstanceId,
        surfaceId: input.surfaceId
      }
    }
    pending.rendererReadySurfaceInstanceId = surface.surfaceInstanceId
    if (!surface.initialDocumentReady) {
      return {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        accepted: false,
        status: 'noop',
        reason: 'not_registered',
        retryable: true,
        requestId: input.requestId,
        surfaceInstanceId: surface.surfaceInstanceId,
        surfaceId: input.surfaceId
      }
    }
    this.completePendingSurfaceRequests()
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      accepted: true,
      status: 'applied',
      reason: 'surface_ready',
      retryable: false,
      requestId: input.requestId,
      surfaceInstanceId: surface.surfaceInstanceId,
      surfaceId: input.surfaceId
    }
  }

  selectManualSurface(
    host: WebContents,
    input: BrowserSurfaceSelectedInput
  ): BrowserSurfaceSelectedOutput {
    this.assertUsable()
    if (input.surfaceId === null) {
      const active = this.activeSurfaceId ? this.surfaces.get(this.activeSurfaceId) : undefined
      if (active && active.host !== host) {
        // A trusted but different Renderer must never clear another host's selection.
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      if (input.selectionRevision < this.rendererSelectionRevision) {
        return this.staleSelectionOutput(input, 'stale_revision', false, null)
      }
      if (input.selectionRevision === this.rendererSelectionRevision) {
        return !this.activeSurfaceId
          ? this.noopSelectionOutput(input, 'selection_unchanged', false, null)
          : this.staleSelectionOutput(input, 'stale_revision', false, null)
      }

      this.rendererSelectionRevision = input.selectionRevision
      if (!this.activeSurfaceId) {
        return this.noopSelectionOutput(input, 'selection_unchanged', false, null)
      }
      this.activeSurfaceId = undefined
      this.manualSelectionRevision += 1
      return this.appliedSelectionOutput(input, null)
    }

    const surface = this.surfaces.get(input.surfaceId)
    if (!surface || surface.guest.isDestroyed()) {
      return this.noopSelectionOutput(input, 'not_registered', true, null)
    }
    if (surface.host !== host) {
      // Host ownership is a security boundary, not a retryable lifecycle condition.
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (this.closingSurfaceIds.has(surface.surfaceId)) {
      return this.staleSelectionOutput(input, 'surface_closing', false, surface.surfaceInstanceId)
    }
    if (input.surfaceInstanceId === null) {
      if (input.selectionRevision <= this.rendererSelectionRevision) {
        return this.staleSelectionOutput(input, 'stale_revision', false, surface.surfaceInstanceId)
      }
      // The probe is deliberately side-effect free. Renderer must prove it is still presenting
      // this exact webview before it echoes the Main-generated incarnation back.
      return this.noopSelectionOutput(input, 'instance_required', true, surface.surfaceInstanceId)
    }
    if (input.surfaceInstanceId !== surface.surfaceInstanceId) {
      return this.staleSelectionOutput(input, 'instance_mismatch', false, surface.surfaceInstanceId)
    }
    if (input.selectionRevision < this.rendererSelectionRevision) {
      return this.staleSelectionOutput(input, 'stale_revision', false, surface.surfaceInstanceId)
    }
    if (input.selectionRevision === this.rendererSelectionRevision) {
      return this.activeSurfaceId === surface.surfaceId
        ? this.noopSelectionOutput(input, 'selection_unchanged', false, surface.surfaceInstanceId)
        : this.staleSelectionOutput(input, 'stale_revision', false, surface.surfaceInstanceId)
    }

    // Advance the Renderer ordering watermark even for a duplicate target. This prevents a
    // delayed lower-revision A -> B intent from overtaking a newer A duplicate, while the separate
    // Main mutation epoch below remains unchanged for idempotent reports and Tool leases.
    this.rendererSelectionRevision = input.selectionRevision
    if (this.activeSurfaceId === surface.surfaceId) {
      return this.noopSelectionOutput(
        input,
        'selection_unchanged',
        false,
        surface.surfaceInstanceId
      )
    }

    // Record the trusted UI choice immediately. An in-flight Playwright operation retains its
    // exact generation and mutation epoch; the next serialized Tool synchronizes currentTab.
    this.manualSelectionRevision += 1
    this.activeSurfaceId = surface.surfaceId
    return this.appliedSelectionOutput(input, surface.surfaceInstanceId)
  }

  private appliedSelectionOutput(
    input: BrowserSurfaceSelectedInput,
    surfaceInstanceId: string | null
  ): BrowserSurfaceSelectedOutput {
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      status: 'applied',
      reason: 'selection_applied',
      retryable: false,
      surfaceId: input.surfaceId,
      surfaceInstanceId,
      selectionRevision: input.selectionRevision,
      authoritativeRevision: this.rendererSelectionRevision
    }
  }

  private noopSelectionOutput(
    input: BrowserSurfaceSelectedInput,
    reason: 'selection_unchanged' | 'instance_required' | 'not_registered',
    retryable: boolean,
    surfaceInstanceId: string | null
  ): BrowserSurfaceSelectedOutput {
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      status: 'noop',
      reason,
      retryable,
      surfaceId: input.surfaceId,
      surfaceInstanceId,
      selectionRevision: input.selectionRevision,
      authoritativeRevision: this.rendererSelectionRevision
    }
  }

  private staleSelectionOutput(
    input: BrowserSurfaceSelectedInput,
    reason: 'stale_revision' | 'instance_mismatch' | 'surface_closing',
    retryable: boolean,
    surfaceInstanceId: string | null
  ): BrowserSurfaceSelectedOutput {
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      status: 'stale',
      reason,
      retryable,
      surfaceId: input.surfaceId,
      surfaceInstanceId,
      selectionRevision: input.selectionRevision,
      authoritativeRevision: this.rendererSelectionRevision
    }
  }

  /** Disconnects automation while keeping the user's page and browser partition alive. */
  async detachAutomation(): Promise<void> {
    this.automationEpoch += 1
    const targetCreation = this.targetCreation
    if (targetCreation) this.finishTargetCreationIntent(targetCreation)
    this.needsReveal = true
    this.networkGuard?.deactivateAutomation()
    this.rejectPendingSurfaceRequests('browser.surface_unavailable')
    for (const admission of this.surfaceGroupAdmissions.values()) admission.release()
    const connecting = this.connecting
    const active = this.active
    this.active = undefined
    this.automationSurfaceId = undefined
    this.toolSurfaceLease = undefined

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
    // The group transport already detached each per-guest debugger. Keep Broker claims bound to
    // their live surface generations so a later task can reconnect without widening admission.
  }

  getActiveSurfaceIdentity(): { generation: number; surfaceId: string } | null {
    const surfaceId = this.currentAutomationSurfaceId()
    if (!this.isActiveAttachmentUsable(this.active) || !surfaceId) return null
    const surface = this.surfaces.get(surfaceId)
    return surface && !surface.guest.isDestroyed()
      ? { generation: surface.generation, surfaceId: surface.surfaceId }
      : null
  }

  /** Returns a stable, non-navigating Main-only document identity without attaching automation. */
  getSensitiveTargetIdentity(): BrowserSensitiveTargetIdentity | null {
    if (this.disposed) return null
    // Approval proposals are created before a Tool lease exists and therefore bind the user's
    // latest trusted UI choice. Once dispatch owns a lease, the identity stays locked even if the
    // user selects another tab for the next call.
    const surfaceId = this.toolSurfaceLease?.surfaceId ?? this.activeSurfaceId
    if (!surfaceId) return null
    const surface = this.surfaces.get(surfaceId)
    if (
      !surface ||
      surface.guest.isDestroyed() ||
      surface.navigationInProgress ||
      surface.loadError ||
      surface.crashError
    ) {
      return null
    }
    const origin = safeHttpOrigin(surface.logicalUrl ?? surface.guest.getURL())
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

  beginTargetCreationIntent(
    intent: ManagedTargetCreationIntent,
    authority?: BrowserTargetCreationAuthority
  ): () => void {
    this.assertUsable()
    if (this.targetCreation !== undefined || (intent === 'interactive' && !authority)) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const targetCreation: ActiveTargetCreationIntent = {
      authority,
      finished: false,
      intent
    }
    this.targetCreation = targetCreation
    if (this.isActiveAttachmentUsable(this.active)) {
      targetCreation.transportFinish = this.active.transport.setTargetCreationIntent(intent)
    }
    return () => this.finishTargetCreationIntent(targetCreation)
  }

  private finishTargetCreationIntent(targetCreation: ActiveTargetCreationIntent): void {
    if (targetCreation.finished) return
    targetCreation.finished = true
    targetCreation.transportFinish?.()
    targetCreation.transportFinish = undefined
    targetCreation.authority?.finish()
    if (this.targetCreation === targetCreation) this.targetCreation = undefined
  }

  async beginToolSurfaceLease(): Promise<BrowserToolSurfaceLease> {
    this.assertUsable()
    if (this.toolSurfaceLease) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = await this.ensureSurface()
    return await this.acquireToolSurfaceLease(surface)
  }

  /**
   * Locks an already-visible tab without creating, revealing, or selecting a page. Context-only
   * tools and browser_tabs use this to preserve the fixed Playwright zero-tab semantics.
   */
  async beginExistingToolSurfaceLease(): Promise<BrowserToolSurfaceLease | null> {
    this.assertUsable()
    if (this.toolSurfaceLease) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surfaceId = this.activeSurfaceId
    if (!surfaceId) return null
    const surface = this.surfaces.get(surfaceId)
    if (!surface || surface.guest.isDestroyed()) return null
    await this.addSurfaceToActiveGroup(surface)
    return await this.acquireToolSurfaceLease(surface)
  }

  /** Locks the exact current index without creating, selecting, or revealing a page. */
  async beginToolSurfaceLeaseByIndex(index: number): Promise<BrowserToolSurfaceLease> {
    this.assertUsable()
    if (this.toolSurfaceLease) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = this.resolveSurfaceSelection({ index })
    await this.addSurfaceToActiveGroup(surface)
    return await this.acquireToolSurfaceLease(surface)
  }

  private async acquireToolSurfaceLease(surface: ManagedSurface): Promise<BrowserToolSurfaceLease> {
    await this.reconcileActiveGroup()
    const index = this.orderedSurfaces().indexOf(surface)
    if (index < 0 || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    const record = {
      generation: surface.generation,
      selectionRevision: this.manualSelectionRevision,
      surfaceId: surface.surfaceId
    }
    this.toolSurfaceLease = record
    this.automationSurfaceId = surface.surfaceId
    let finished = false
    return {
      ...record,
      index,
      closeSurface: async () => {
        if (finished || this.toolSurfaceLease !== record) {
          throw new BrowserSurfaceManagerError('browser.surface_unavailable')
        }
        const current = this.surfaces.get(record.surfaceId)
        if (!current || current.generation !== record.generation || current.guest.isDestroyed()) {
          throw new BrowserSurfaceManagerError('browser.target_closed')
        }
        await this.closeSurface(record.surfaceId)
      },
      finish: () => {
        if (finished) return
        finished = true
        if (this.toolSurfaceLease === record) this.toolSurfaceLease = undefined
      },
      printToPdf: async () => {
        if (finished || this.toolSurfaceLease !== record) {
          throw new BrowserSurfaceManagerError('browser.surface_unavailable')
        }
        return await this.printExactSurfaceToPdf(record)
      },
      resolveIndex: async () => await this.resolveToolSurfaceIndex(record),
      resizeSurface: async (input) => {
        if (finished || this.toolSurfaceLease !== record) {
          throw new BrowserSurfaceManagerError('browser.surface_unavailable')
        }
        return await this.resizeExactSurface(record, input)
      }
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
    return this.toSurfaceView(await this.createSurfaceRecord(input, true))
  }

  /**
   * Materializes the single user-visible page that represents a zero-tab `browser_tabs new`.
   * The exact run/call authority is claimed while the guest is blank. Host must connect the fixed
   * MCP Context before navigating this Page so dialogs, popups, routes, downloads, and currentTab
   * all observe the same official lifecycle. It must not issue a second Target.createTarget.
   */
  async createInitialTargetSurface(input: {
    authority: BrowserTargetCreationAuthority
  }): Promise<BrowserSurfaceView> {
    this.assertUsable()
    if (
      input.authority.action !== 'new' ||
      this.surfaces.size > 0 ||
      this.pendingCreatedSurfaceCount() > 0
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    let surface: ManagedSurface | undefined
    try {
      surface = await this.createSurfaceRecord({ activate: true }, false)
      await loadManagedBlankSurface(surface, this.attachTimeoutMs)
      await input.authority.claim({
        generation: surface.generation,
        guest: surface.guest,
        surfaceId: surface.surfaceId
      })
      await this.addSurfaceToActiveGroup(surface, true)
      return this.toSurfaceView(surface)
    } catch (error) {
      if (surface && this.surfaces.get(surface.surfaceId) === surface) {
        try {
          await this.closeSurface(surface.surfaceId)
        } catch {
          this.handleTargetClosed(surface.surfaceId, surface.generation, surface.guest)
        }
      }
      throw error
    }
  }

  private async createSurfaceRecord(
    input: { activate?: boolean; url?: string },
    attachToActiveGroup: boolean,
    reserveDeferredAdmission = true
  ): Promise<ManagedSurface> {
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
    const deferInitialAdmission = !attachToActiveGroup || input.url !== undefined
    if (deferInitialAdmission) this.suppressAutomaticGroupAdmission.add(surfaceId)
    if (deferInitialAdmission && !reserveDeferredAdmission) {
      this.suppressDeferredGroupAdmissionReservation.add(surfaceId)
    }
    let surface: ManagedSurface | undefined
    try {
      surface = await this.requestSurface(
        host,
        'createSurface',
        surfaceId,
        undefined,
        shouldActivate
      )
      let admittedBeforeNavigation = false
      if (
        input.url !== undefined &&
        attachToActiveGroup &&
        this.isActiveAttachmentUsable(this.active)
      ) {
        // Attach the inert blank page first so group-scoped route/offline/trace state observes the
        // very first requested URL. The bootstrap marker never enters the Playwright Context.
        await loadManagedBlankSurface(surface, this.attachTimeoutMs)
        await this.addSurfaceToActiveGroup(surface, true)
        admittedBeforeNavigation = true
      }
      if (input.url === 'about:blank' && !admittedBeforeNavigation) {
        await loadManagedBlankSurface(surface, this.attachTimeoutMs)
      } else if (input.url !== undefined && input.url !== 'about:blank') {
        await loadManagedSurface(surface, input.url, this.attachTimeoutMs)
      }
      if (input.activate !== false) await this.switchActiveSurface(surface)
      if (attachToActiveGroup && !admittedBeforeNavigation) {
        await this.addSurfaceToActiveGroup(surface)
      }
      return surface
    } catch (error) {
      if (surface && this.surfaces.get(surface.surfaceId) === surface) {
        try {
          await this.closeSurface(surface.surfaceId)
        } catch {
          this.handleTargetClosed(surface.surfaceId, surface.generation, surface.guest)
        }
      }
      throw error
    } finally {
      this.suppressAutomaticGroupAdmission.delete(surfaceId)
      this.suppressDeferredGroupAdmissionReservation.delete(surfaceId)
    }
  }

  async selectSurface(input: { index?: number; surfaceId?: string }): Promise<BrowserSurfaceView> {
    this.assertUsable()
    const surface = this.resolveSurfaceSelection(input)
    const host = this.resolveHost()
    if (!host || host.isDestroyed() || surface.host !== host) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (surface.nativePopup) {
      surface.nativePopup.window.show()
      surface.nativePopup.window.focus()
    } else {
      await this.requestSurface(host, 'selectSurface', surface.surfaceId)
    }
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
    return await this.resizeExactSurface(identity, input)
  }

  private async resizeExactSurface(
    identity: { generation: number; surfaceId: string },
    input: { height: number; width: number }
  ): Promise<{ height: number; width: number }> {
    const surface = this.surfaces.get(identity.surfaceId)
    const host = this.resolveHost()
    if (
      !surface ||
      surface.generation !== identity.generation ||
      !host ||
      host.isDestroyed() ||
      surface.host !== host
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const dimensions = normalizeViewportSize(input)
    if (surface.nativePopup) {
      surface.nativePopup.window.setContentSize(dimensions.width, dimensions.height)
      const [width, height] = surface.nativePopup.window.getContentSize()
      return { width, height }
    }
    await this.requestSurface(host, 'resizeSurface', surface.surfaceId, dimensions)
    return dimensions
  }

  private async printExactSurfaceToPdf(identity: {
    generation: number
    surfaceId: string
  }): Promise<Uint8Array> {
    const surface = this.surfaces.get(identity.surfaceId)
    if (!surface || surface.generation !== identity.generation || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    try {
      const bytes = await printManagedGuestToPdf(
        surface.guest,
        { printBackground: false, margins: { top: 0, bottom: 0, left: 0, right: 0 } },
        { timeoutMs: this.attachTimeoutMs }
      )
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
    }
  }

  /** Requests removal of a Browser tab. It never closes the Captain Who BrowserWindow. */
  async closeSurface(surfaceId?: string): Promise<void> {
    this.assertUsable()
    const targetSurfaceId = surfaceId ?? this.activeSurfaceId
    if (!targetSurfaceId) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    const surface = this.surfaces.get(targetSurfaceId)
    if (!surface) throw new BrowserSurfaceManagerError('browser.target_closed')
    if (surface.nativePopup) {
      this.closingSurfaceIds.add(targetSurfaceId)
      surface.nativePopup.window.destroy()
      return
    }
    await this.enqueueRendererCommand(async () => {
      if (this.surfaces.get(targetSurfaceId) !== surface || surface.guest.isDestroyed()) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }
      if (this.closeWaiters.has(targetSurfaceId)) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      const requestId = randomUUID()
      this.closingSurfaceIds.add(targetSurfaceId)
      this.clearActiveSelectionForRetiredSurface(targetSurfaceId)
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
          surfaceInstanceId: surface.surfaceInstanceId,
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

    const restorePopupOpener = surface.nativePopup && this.activeSurfaceId === surfaceId
    surface.nativePopup?.cleanup()
    surface.dispatchFence?.finishSilently()
    surface.guest.removeListener('destroyed', surface.handleDestroyed)
    this.removeNavigationListeners(surface)
    this.surfaces.delete(surfaceId)
    const admission = this.surfaceGroupAdmissions.get(
      `${surface.surfaceId}\u0000${surface.generation}`
    )
    admission?.release()
    this.active?.transport.removeSurface(surfaceId, generation, false)
    this.broker.releaseSurface(surfaceId, generation)
    void this.releaseSurfaceResources?.({ surfaceId, generation }).catch(() => undefined)
    const closeWaiter = this.closeWaiters.get(surfaceId)
    if (closeWaiter?.generation === generation) {
      clearTimeout(closeWaiter.timer)
      this.closeWaiters.delete(surfaceId)
      closeWaiter.resolve()
    }
    const wasExplicitlyClosing = this.closingSurfaceIds.delete(surfaceId)
    const awaitingReplacement =
      !surface.nativePopup &&
      !this.disposed &&
      !wasExplicitlyClosing &&
      !surface.host.isDestroyed() &&
      this.schedulePendingSurfaceHandoff(surface)
    if (!awaitingReplacement) {
      this.rejectPendingSurfaceRequestsForSurface(
        surfaceId,
        'browser.target_closed',
        'target_closed'
      )
    }
    // Renderer owns tab ordering and fallback choice. Main clears the retired exact identity and
    // waits for a bound Renderer selection instead of guessing from a potentially stale snapshot.
    this.clearActiveSelectionForRetiredSurface(surfaceId)
    if (restorePopupOpener && surface.nativePopup) {
      const opener = this.surfaces.get(surface.nativePopup.openerSurfaceId)
      if (
        !this.disposed &&
        opener &&
        opener.generation === surface.nativePopup.openerGeneration &&
        !opener.guest.isDestroyed() &&
        !opener.host.isDestroyed() &&
        !this.closingSurfaceIds.has(opener.surfaceId)
      ) {
        this.activeSurfaceId = opener.surfaceId
        this.needsReveal = false
      }
    }
    if (surface.nativePopup) {
      // Transport failure can retire a still-live popup. The removed surface must not leave an
      // untracked native window behind; destruction re-entry sees that this identity is gone.
      if (!surface.nativePopup.window.isDestroyed()) surface.nativePopup.window.destroy()
      return
    }
    if (
      !awaitingReplacement &&
      !this.disposed &&
      !wasExplicitlyClosing &&
      !surface.host.isDestroyed()
    ) {
      void this.enqueueRendererCommand(async () => {
        // A StrictMode remount can register a new generation with the same Renderer page identity
        // on the next turn. Never let an old generation close its replacement.
        await nextEventLoopTurn()
        if (this.disposed || surface.host.isDestroyed() || this.surfaces.has(surfaceId)) return
        this.sendCommand(surface.host, {
          schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
          kind: 'closeSurface',
          requestId: randomUUID(),
          surfaceInstanceId: surface.surfaceInstanceId,
          surfaceId
        })
        // There is no guest left to acknowledge unexpected-target cleanup. Keep the FIFO busy
        // through the next event-loop turn so a later scalar Renderer command cannot replace the
        // close notification in the same React batch.
        await nextEventLoopTurn()
      }).catch(() => undefined)
    }
  }

  private forceRetireSurface(input: { generation: number; surfaceId: string }): void {
    const surface = this.surfaces.get(input.surfaceId)
    if (!surface || surface.generation !== input.generation) return
    if (surface.nativePopup) {
      surface.nativePopup.window.destroy()
      return
    }
    this.handleTargetClosed(surface.surfaceId, surface.generation, surface.guest)
    // Unlike the ordinary destroyed event, this path can retire a still-live guest after a lost
    // Renderer close acknowledgement. Remove its Broker registration/listeners immediately, then
    // destroy the exact hidden control guest so NetworkGuard/DownloadBroker destroyed listeners
    // cannot retain an untracked WebContents. This fallback is intentionally limited to the exact
    // context-control generation supplied by the private group transport.
    this.broker.unregisterManagedGuest({ guest: surface.guest, host: surface.host })
    if (!surface.guest.isDestroyed()) {
      try {
        surface.guest.close({ waitForBeforeUnload: false })
      } catch {
        // Logical retirement already removed every Main-owned capability route. Electron may
        // throw while a renderer crash is concurrently destroying the same WebContents.
      }
    }
  }

  /** Adopt Electron's original popup WebContents so Chromium retains its real opener semantics. */
  canCreateNativePopup(input: { guest: WebContents; url: string }): boolean {
    const source = [...this.surfaces.values()].find((surface) => surface.guest === input.guest)
    return Boolean(
      !this.disposed &&
      this.networkGuard &&
      source &&
      !source.guest.isDestroyed() &&
      !source.host.isDestroyed() &&
      !this.closingSurfaceIds.has(source.surfaceId) &&
      (input.url === '' || input.url === 'about:blank' || isSafeManagedPageUrl(input.url)) &&
      this.surfaces.size + this.pendingCreatedSurfaceCount() < this.maxSurfaces &&
      (Date.now() - this.popupWindowStartedAt >= 1_000 || this.popupCount < MAX_POPUPS_PER_SECOND)
    )
  }

  createNativePopup(input: {
    guest: WebContents
    url: string
    options: BrowserWindowConstructorOptions
    createWindow: (options: BrowserWindowConstructorOptions) => BrowserWindow
    configureGuest: (guest: WebContents) => void
  }): WebContents {
    this.assertUsable()
    const source = [...this.surfaces.values()].find((surface) => surface.guest === input.guest)
    if (
      !this.networkGuard ||
      !source ||
      source.guest.isDestroyed() ||
      source.host.isDestroyed() ||
      this.closingSurfaceIds.has(source.surfaceId) ||
      (input.url !== '' && input.url !== 'about:blank' && !isSafeManagedPageUrl(input.url))
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const now = Date.now()
    if (now - this.popupWindowStartedAt >= 1_000) {
      this.popupWindowStartedAt = now
      this.popupCount = 0
    }
    if (
      ++this.popupCount > MAX_POPUPS_PER_SECOND ||
      this.surfaces.size + this.pendingCreatedSurfaceCount() >= this.maxSurfaces
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }
    const surfaceId = this.allocateSurfaceId()
    const window = input.createWindow(input.options)
    const guest = window.webContents
    const loading = observeNativePopupLoad(guest)
    const close = (): void => {
      if (!window.isDestroyed()) window.destroy()
    }
    this.suppressAutomaticGroupAdmission.add(surfaceId)
    try {
      input.configureGuest(guest)
      this.registerManagedGuest({
        documentReady: true,
        guest,
        host: source.host,
        partition: BROWSER_WEBVIEW_PARTITION,
        surfaceId,
        nativePopup: { window, openerGuest: source.guest }
      })
      const popup = this.surfaces.get(surfaceId)!
      const onFocus = (): void => {
        if (this.surfaces.get(surfaceId) !== popup || this.activeSurfaceId === surfaceId) return
        this.activeSurfaceId = surfaceId
        this.manualSelectionRevision += 1
      }
      window.on('focus', onFocus)
      source.guest.once('destroyed', close)
      popup.nativePopup!.cleanup = () => {
        loading.dispose()
        window.removeListener('focus', onFocus)
        source.guest.removeListener('destroyed', close)
      }
      // This call installs its exact-child gate synchronously. Electron may initiate the original
      // navigation/POST as soon as createWindow returns; no request may precede owner/route setup.
      const ready = this.networkGuard.beginNativePopupAdmission({
        guest,
        opener: source.guest,
        url: input.url,
        prepare: async () => {
          await this.addSurfaceToActiveGroup(popup, true)
        },
        settled: loading.settled,
        close
      })
      void ready.then(() => {
        if (window.isDestroyed()) return
        loading.start()
        window.showInactive()
      }, close)
      return guest
    } catch (error) {
      loading.dispose()
      close()
      throw error
    } finally {
      this.suppressAutomaticGroupAdmission.delete(surfaceId)
    }
  }

  /** Creates a denied Electron popup without disrupting the still-running opener tool call. */
  async handlePopup(input: {
    authority?: BrowserTargetCreationAuthority
    guest: WebContents
    url: string
  }): Promise<void> {
    this.assertUsable()
    if (input.guest.isDestroyed() || !isSafeManagedPageUrl(input.url)) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const source = [...this.surfaces.values()].find((surface) => surface.guest === input.guest)
    if (!source) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
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
    if (!input.authority && !this.isActiveAttachmentUsable(this.active)) {
      const popup = await this.createSurface({ activate: false, url: input.url })
      if (popup.isActive) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      return
    }
    let popup: ManagedSurface | undefined
    try {
      popup = await this.createSurfaceRecord({ activate: false }, false)
      await loadManagedBlankSurface(popup, this.attachTimeoutMs)
      await input.authority?.claim({
        generation: popup.generation,
        guest: popup.guest,
        surfaceId: popup.surfaceId
      })
      // When a connection already exists, group-wide route/offline/trace state is installed before
      // the first popup navigation. During a zero-tab bootstrap the exact guest is still claimed
      // before load and the subsequent Context connection reconciles it into the same private group.
      await this.addSurfaceToActiveGroup(popup, true)
      await loadManagedSurface(popup, input.url, this.attachTimeoutMs)
      if (this.activeSurfaceId === popup.surfaceId) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
    } catch (error) {
      if (popup && this.surfaces.get(popup.surfaceId) === popup) {
        try {
          await this.closeSurface(popup.surfaceId)
        } catch {
          this.handleTargetClosed(popup.surfaceId, popup.generation, popup.guest)
        }
      }
      throw error
    }
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    for (const handoff of this.pendingSurfaceHandoffs.values()) clearTimeout(handoff.timer)
    this.pendingSurfaceHandoffs.clear()
    this.rejectPendingSurfaceRequests('browser.manager_shutdown', 'request_cancelled')
    this.settledSurfaceRequests.clear()
    let detachError: unknown
    try {
      await this.detachAutomation()
    } catch (error) {
      detachError = error
    }
    const remainingSurfaces = [...this.surfaces.values()]
    const releaseSurfaceResources = remainingSurfaces.map((surface) =>
      this.releaseSurfaceResources?.({
        surfaceId: surface.surfaceId,
        generation: surface.generation
      })
    )
    for (const surface of remainingSurfaces) {
      surface.nativePopup?.cleanup()
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
    // Revocation/detach intentionally preserves manual pages. Full manager shutdown is the app
    // lifecycle boundary: close every exact remaining guest so webviews, renderer processes, and
    // their non-Manager listeners cannot survive after the host has discarded its surface map.
    for (const surface of remainingSurfaces) {
      if (surface.guest.isDestroyed()) continue
      try {
        if (surface.nativePopup) surface.nativePopup.window.destroy()
        else surface.guest.close({ waitForBeforeUnload: false })
      } catch {
        // Electron will also destroy these guests with their owning BrowserWindow. All logical
        // admission and debugger state has already been retired above, so shutdown remains safe.
      }
    }
    await Promise.allSettled([
      this.networkGuard?.shutdown(),
      this.internalPageStore.shutdown(),
      ...releaseSurfaceResources
    ])
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

  private async connectBrowserContext(
    expectedEpoch: number,
    createVisiblePage: boolean
  ): Promise<BrowserContext> {
    const surface = createVisiblePage ? await this.ensureSurface() : undefined
    this.assertAttachmentEpoch(expectedEpoch)
    if (
      surface &&
      (this.surfaces.get(surface.surfaceId) !== surface || surface.guest.isDestroyed())
    ) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }

    let transport: ElectronSurfaceGroupCdpTransport | undefined
    let browser: Browser | undefined
    try {
      // Preserve stable UI tab indexes. The fixed MCP Context records pages in attached event
      // order; selecting the current UI tab is synchronized separately through bringToFront.
      const ordered = this.orderedSurfaces()
      for (const candidate of ordered) {
        if (parseBrowserSurfaceBootstrapUrl(candidate.guest.getURL()) === candidate.surfaceId) {
          await loadManagedBlankSurface(candidate, this.attachTimeoutMs)
        }
      }
      transport = await this.broker.connectSurfaceGroup(
        ordered.map((candidate) => ({
          generation: candidate.generation,
          surfaceId: candidate.surfaceId
        })),
        {
          activateSurface: async (identity) => {
            const candidate = this.surfaces.get(identity.surfaceId)
            if (
              !candidate ||
              candidate.generation !== identity.generation ||
              candidate.guest.isDestroyed()
            ) {
              throw new BrowserSurfaceManagerError('browser.target_closed')
            }
            await this.activateSurfaceFromAutomation(candidate)
          },
          closeSurface: async (identity) => {
            const candidate = this.surfaces.get(identity.surfaceId)
            const lease = this.toolSurfaceLease
            if (
              !candidate ||
              candidate.generation !== identity.generation ||
              (lease !== undefined &&
                (lease.surfaceId !== identity.surfaceId ||
                  lease.generation !== identity.generation))
            ) {
              throw new BrowserSurfaceManagerError('browser.target_closed')
            }
            await this.closeSurface(identity.surfaceId)
          },
          createSurface: async ({ intent, purpose, url }) => {
            // Renderer creates the guest in the background. The group transport performs the
            // single authoritative activation only for an interactive Target-creation intent.
            // Capture one exact record before any Renderer await. A cancelled old call cannot
            // silently skip its claim or borrow a later call's authority (ABA).
            const targetCreation = purpose === 'target' ? this.targetCreation : undefined
            if (
              purpose === 'target' &&
              (!targetCreation ||
                targetCreation.finished ||
                targetCreation.intent !== intent ||
                (intent === 'interactive' && !targetCreation.authority))
            ) {
              throw new BrowserSurfaceManagerError('browser.surface_unavailable')
            }
            const authority = targetCreation?.authority
            let candidate: ManagedSurface | undefined
            try {
              if (
                purpose === 'context-control' &&
                (intent !== 'background' || url !== 'about:blank')
              ) {
                throw new BrowserSurfaceManagerError('browser.surface_unavailable')
              }
              candidate = await this.createSurfaceRecord(
                { activate: false },
                false,
                purpose !== 'context-control'
              )
              // Claim run/call/capability ownership while the new guest is still inert. Download
              // destinations and exact network ownership must exist before its first real URL.
              await loadManagedBlankSurface(candidate, this.attachTimeoutMs)
              if (purpose === 'context-control') {
                const transport = await this.broker.connect(
                  candidate.surfaceId,
                  candidate.generation
                )
                return {
                  generation: candidate.generation,
                  surfaceId: candidate.surfaceId,
                  transport
                }
              }
              if (
                targetCreation &&
                (targetCreation.finished || this.targetCreation !== targetCreation)
              ) {
                throw new BrowserSurfaceManagerError('browser.surface_unavailable')
              }
              await authority?.claim({
                generation: candidate.generation,
                guest: candidate.guest,
                surfaceId: candidate.surfaceId
              })
              if (
                targetCreation &&
                (targetCreation.finished || this.targetCreation !== targetCreation)
              ) {
                throw new BrowserSurfaceManagerError('browser.surface_unavailable')
              }
              // Registration reserved this surface's createdSequence slot. Releasing it only
              // after ownership is ready admits B before any later manual/popup C.
              await this.addSurfaceToActiveGroup(candidate, true)
              if (
                targetCreation &&
                (targetCreation.finished || this.targetCreation !== targetCreation)
              ) {
                throw new BrowserSurfaceManagerError('browser.surface_unavailable')
              }
              if (url !== 'about:blank') {
                await loadManagedSurface(candidate, url, this.attachTimeoutMs)
              }
              return {
                generation: candidate.generation,
                surfaceId: candidate.surfaceId
              }
            } catch (error) {
              if (candidate && this.surfaces.get(candidate.surfaceId) === candidate) {
                try {
                  await this.closeSurface(candidate.surfaceId)
                } catch {
                  this.handleTargetClosed(
                    candidate.surfaceId,
                    candidate.generation,
                    candidate.guest
                  )
                }
              }
              throw error
            }
          },
          forceRetireSurface: (identity) => {
            this.forceRetireSurface(identity)
          },
          getActiveSurfaceId: () => this.automationSurfaceId ?? this.activeSurfaceId,
          handleSurfaceTransportClosed: (identity) => {
            this.handleTargetClosed(identity.surfaceId, identity.generation)
          }
        }
      )
      const targetCreation = this.targetCreation
      if (targetCreation && !targetCreation.finished) {
        targetCreation.transportFinish = transport.setTargetCreationIntent(targetCreation.intent)
      }
      this.assertAttachmentEpoch(expectedEpoch)
      browser = await this.connectOverCdp(transport)
      this.assertAttachmentEpoch(expectedEpoch)
      const contexts = browser.contexts()
      if (contexts.length !== 1 || !contexts[0]) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      if (
        surface &&
        (this.surfaces.get(surface.surfaceId) !== surface || surface.guest.isDestroyed())
      ) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }
      const attachment: ActiveAttachment = {
        browser,
        context: contexts[0],
        transport
      }
      this.active = attachment
      this.automationSurfaceId = ordered[0]?.surfaceId
      browser.once('disconnected', () => {
        if (this.active === attachment) this.active = undefined
      })
      // A guest may register after the initial ordered snapshot but before the Browser connection
      // is published. Reconcile under the same serialized admission gate so Context._tabs remains
      // in stable createdSequence order and no managed tab is omitted.
      await this.reconcileActiveGroup(attachment)
      return attachment.context
    } catch (error) {
      transport?.close()
      if (error instanceof BrowserSurfaceManagerError) throw error
      throw new BrowserSurfaceManagerError(
        surface?.guest.isDestroyed() ? 'browser.target_closed' : 'browser.surface_unavailable'
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
    const leasedSurface = this.toolSurfaceLease
      ? this.surfaces.get(this.toolSurfaceLease.surfaceId)
      : undefined
    if (
      leasedSurface &&
      leasedSurface.generation === this.toolSurfaceLease?.generation &&
      !leasedSurface.guest.isDestroyed()
    ) {
      await this.addSurfaceToActiveGroup(leasedSurface)
      return leasedSurface
    }
    const activeSurface = this.activeSurfaceId ? this.surfaces.get(this.activeSurfaceId) : undefined
    if (activeSurface && !activeSurface.guest.isDestroyed()) {
      await this.addSurfaceToActiveGroup(activeSurface)
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
      if (reusable.surfaceId === this.activeSurfaceId && !this.needsReveal) {
        await this.addSurfaceToActiveGroup(reusable)
        return reusable
      }
      if (reusable.nativePopup) {
        reusable.nativePopup.window.show()
        reusable.nativePopup.window.focus()
        this.activeSurfaceId = reusable.surfaceId
        this.needsReveal = false
        await this.addSurfaceToActiveGroup(reusable)
        return reusable
      }
      const revealed = await this.requestSurface(host, 'selectSurface', reusable.surfaceId)
      this.activeSurfaceId = revealed.surfaceId
      this.needsReveal = false
      await this.addSurfaceToActiveGroup(revealed)
      return revealed
    }

    if (this.surfaces.size + this.pendingCreatedSurfaceCount() >= this.maxSurfaces) {
      throw new BrowserSurfaceManagerError('browser.surface_capacity_exceeded')
    }
    const surfaceId = this.allocateSurfaceId()
    const created = await this.requestSurface(host, 'ensureAttached', surfaceId)
    this.activeSurfaceId = created.surfaceId
    this.needsReveal = false
    await this.addSurfaceToActiveGroup(created)
    return created
  }

  private addSurfaceToActiveGroup(surface: ManagedSurface, releaseDeferred = false): Promise<void> {
    const attachment = this.active
    if (!this.isActiveAttachmentUsable(attachment)) return Promise.resolve()
    if (attachment.transport.hasSurface(surface.surfaceId, surface.generation)) {
      return Promise.resolve()
    }
    const key = `${surface.surfaceId}\u0000${surface.generation}`
    const pending = this.surfaceGroupAdmissions.get(key)
    if (pending) {
      if (pending.attachment !== attachment) {
        pending.release()
        return pending.attempt
          .catch(() => undefined)
          .then(() => this.addSurfaceToActiveGroup(surface, releaseDeferred))
      }
      if (releaseDeferred) pending.release()
      return pending.attempt
    }
    const admission = this.reserveSurfaceGroupAdmission(surface, false)
    return admission?.attempt ?? Promise.resolve()
  }

  private reserveSurfaceGroupAdmission(
    surface: ManagedSurface,
    deferred: boolean
  ): PendingSurfaceGroupAdmission | undefined {
    const attachment = this.active
    if (!this.isActiveAttachmentUsable(attachment)) return undefined
    if (attachment.transport.hasSurface(surface.surfaceId, surface.generation)) return undefined
    const key = `${surface.surfaceId}\u0000${surface.generation}`
    const existing = this.surfaceGroupAdmissions.get(key)
    if (existing) return existing
    let released = !deferred
    let releaseReady = (): void => undefined
    const ready = deferred
      ? new Promise<void>((resolve) => {
          releaseReady = resolve
        })
      : Promise.resolve()
    const release = (): void => {
      if (released) return
      released = true
      releaseReady()
    }
    const attempt = this.groupAdmissionTail.then(async () => {
      await ready
      await waitForInitialDocumentReady(surface, this.attachTimeoutMs)
      if (this.active !== attachment || !this.isActiveAttachmentUsable(attachment)) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      const current = this.surfaces.get(surface.surfaceId)
      if (
        current !== surface ||
        current.generation !== surface.generation ||
        current.guest.isDestroyed()
      ) {
        throw new BrowserSurfaceManagerError('browser.target_closed')
      }
      if (attachment.transport.hasSurface(surface.surfaceId, surface.generation)) return
      await this.broker.addSurfaceToGroup(attachment.transport, {
        generation: surface.generation,
        surfaceId: surface.surfaceId
      })
    })
    const admission: PendingSurfaceGroupAdmission = {
      attachment,
      attempt,
      deferred,
      get released() {
        return released
      },
      release
    }
    this.groupAdmissionTail = attempt.then(
      () => undefined,
      () => undefined
    )
    this.surfaceGroupAdmissions.set(key, admission)
    const cleanup = (): void => {
      if (this.surfaceGroupAdmissions.get(key) === admission) {
        this.surfaceGroupAdmissions.delete(key)
      }
    }
    // Unlike Promise.finally(), this creates no rejected derived Promise when admission fails.
    void attempt.then(cleanup, cleanup)
    return admission
  }

  private async reconcileActiveGroup(expected = this.active): Promise<void> {
    if (!this.isActiveAttachmentUsable(expected) || this.active !== expected) return
    for (let pass = 0; pass <= this.maxSurfaces; pass += 1) {
      for (const surface of this.orderedSurfaces()) {
        await this.addSurfaceToActiveGroup(surface)
      }
      const current = this.orderedSurfaces()
      if (
        this.active === expected &&
        current.every((surface) =>
          expected.transport.hasSurface(surface.surfaceId, surface.generation)
        )
      ) {
        return
      }
    }
    throw new BrowserSurfaceManagerError('browser.surface_unavailable')
  }

  private async resolveToolSurfaceIndex(record: {
    generation: number
    selectionRevision: number
    surfaceId: string
  }): Promise<number> {
    if (this.toolSurfaceLease !== record) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    const surface = this.surfaces.get(record.surfaceId)
    if (!surface || surface.generation !== record.generation || surface.guest.isDestroyed()) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    await this.reconcileActiveGroup()
    const index = this.orderedSurfaces().indexOf(surface)
    if (index < 0) throw new BrowserSurfaceManagerError('browser.target_closed')
    if (
      this.isActiveAttachmentUsable(this.active) &&
      !this.active.transport.hasSurface(record.surfaceId, record.generation)
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    return index
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

    // Renderer holds one scalar command. If a different command for the same logical surface is
    // ever issued before an older one settles, the newer command is authoritative; retain a
    // bounded tombstone so a queued acknowledgement for the displaced request receives a typed
    // stale result instead of reviving work the Renderer can no longer be presenting.
    for (const superseded of [...this.pendingSurfaceRequests.values()]) {
      if (!superseded.settled && superseded.host === host && superseded.surfaceId === surfaceId) {
        this.rejectPendingSurfaceRequest(
          superseded.requestId,
          'browser.surface_unavailable',
          'request_superseded'
        )
      }
    }

    const requestId = randomUUID()
    let resolve!: (surface: ManagedSurface) => void
    let reject!: (error: BrowserSurfaceManagerError) => void
    const promise = new Promise<ManagedSurface>((promiseResolve, promiseReject) => {
      resolve = promiseResolve
      reject = promiseReject
    })
    const timer = setTimeout(() => {
      this.rejectPendingSurfaceRequest(requestId, 'browser.surface_unavailable', 'request_expired')
    }, this.attachTimeoutMs)
    const pending: PendingEnsure = {
      ...(kind === 'createSurface' ? { activate } : {}),
      ...(dimensions ? { dimensions } : {}),
      host,
      kind,
      promise,
      reject,
      requestId,
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
      this.rejectPendingSurfaceRequest(
        requestId,
        'browser.surface_unavailable',
        'request_cancelled'
      )
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
      if (pending.settled) continue
      const surface = this.surfaces.get(pending.surfaceId)
      if (
        !surface ||
        !surface.initialDocumentReady ||
        pending.rendererReadySurfaceInstanceId === undefined ||
        surface.surfaceInstanceId !== pending.rendererReadySurfaceInstanceId ||
        surface.host !== pending.host ||
        surface.guest.isDestroyed()
      ) {
        continue
      }

      // Main document readiness proves that the guest can be automated; the Renderer binding
      // proves that this exact incarnation is still the webview presented by the right sidebar.
      // Both are required so a StrictMode probe generation can never receive the first navigation.
      pending.settled = true
      clearTimeout(pending.timer)
      this.rememberSettledSurfaceRequest(pending, 'already_ready')
      this.pendingSurfaceRequests.delete(pending.requestId)
      pending.resolve(surface)
    }
  }

  private rejectPendingSurfaceRequest(
    requestId: string,
    code: BrowserSurfaceManagerErrorCode,
    readyReason: Exclude<SettledSurfaceReadyReason, 'already_ready'> = 'request_cancelled'
  ): void {
    const pending = this.pendingSurfaceRequests.get(requestId)
    if (!pending || pending.settled) return
    pending.settled = true
    clearTimeout(pending.timer)
    this.rememberSettledSurfaceRequest(pending, readyReason)
    this.pendingSurfaceRequests.delete(requestId)
    pending.reject(new BrowserSurfaceManagerError(code))
  }

  private rejectPendingSurfaceRequests(
    code: BrowserSurfaceManagerErrorCode,
    readyReason: Exclude<SettledSurfaceReadyReason, 'already_ready'> = 'request_cancelled'
  ): void {
    for (const requestId of [...this.pendingSurfaceRequests.keys()]) {
      this.rejectPendingSurfaceRequest(requestId, code, readyReason)
    }
  }

  private rejectPendingSurfaceRequestsForSurface(
    surfaceId: string,
    code: BrowserSurfaceManagerErrorCode,
    readyReason: Exclude<SettledSurfaceReadyReason, 'already_ready'> = 'request_cancelled'
  ): void {
    for (const pending of [...this.pendingSurfaceRequests.values()]) {
      if (pending.surfaceId === surfaceId) {
        this.rejectPendingSurfaceRequest(pending.requestId, code, readyReason)
      }
    }
  }

  private rememberSettledSurfaceRequest(
    pending: PendingEnsure,
    reason: SettledSurfaceReadyReason
  ): void {
    this.settledSurfaceRequests.delete(pending.requestId)
    this.settledSurfaceRequests.set(pending.requestId, {
      ...(pending.dimensions ? { dimensions: pending.dimensions } : {}),
      host: pending.host,
      kind: pending.kind,
      reason,
      requestId: pending.requestId,
      ...(pending.rendererReadySurfaceInstanceId
        ? { surfaceInstanceId: pending.rendererReadySurfaceInstanceId }
        : {}),
      surfaceId: pending.surfaceId
    })
    while (this.settledSurfaceRequests.size > MAX_SETTLED_SURFACE_REQUESTS) {
      const oldest = this.settledSurfaceRequests.keys().next().value
      if (typeof oldest !== 'string') break
      this.settledSurfaceRequests.delete(oldest)
    }
  }

  private settledSurfaceReadyOutput(
    host: WebContents,
    input: BrowserSurfaceReadyInput
  ): BrowserSurfaceReadyOutput {
    const settled = this.settledSurfaceRequests.get(input.requestId)
    if (
      !settled ||
      settled.host !== host ||
      settled.requestId !== input.requestId ||
      settled.surfaceId !== input.surfaceId ||
      !this.readyViewportMatches(settled, input)
    ) {
      // Unknown identities, cross-host acknowledgements, and payloads that do not match the
      // command Main originally issued remain hard boundary violations.
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    if (settled.reason === 'already_ready') {
      if (
        input.surfaceInstanceId !== undefined &&
        settled.surfaceInstanceId !== input.surfaceInstanceId
      ) {
        return {
          schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
          accepted: false,
          status: 'stale',
          reason: 'instance_mismatch',
          retryable: true,
          requestId: input.requestId,
          ...(settled.surfaceInstanceId
            ? { surfaceInstanceId: settled.surfaceInstanceId }
            : input.surfaceInstanceId
              ? { surfaceInstanceId: input.surfaceInstanceId }
              : {}),
          surfaceId: input.surfaceId
        }
      }
      return {
        schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
        accepted: false,
        status: 'noop',
        reason: 'already_ready',
        retryable: false,
        requestId: input.requestId,
        ...(settled.surfaceInstanceId
          ? { surfaceInstanceId: settled.surfaceInstanceId }
          : input.surfaceInstanceId
            ? { surfaceInstanceId: input.surfaceInstanceId }
            : {}),
        surfaceId: input.surfaceId
      }
    }
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      accepted: false,
      status: 'stale',
      reason: settled.reason,
      retryable: false,
      requestId: input.requestId,
      ...(input.surfaceInstanceId ? { surfaceInstanceId: input.surfaceInstanceId } : {}),
      surfaceId: input.surfaceId
    }
  }

  private readyViewportMatches(
    expected: Pick<PendingEnsure, 'dimensions' | 'kind'>,
    input: BrowserSurfaceReadyInput
  ): boolean {
    if (expected.kind !== 'resizeSurface') return input.viewport === undefined
    return (
      expected.dimensions?.height === input.viewport?.height &&
      expected.dimensions?.width === input.viewport?.width
    )
  }

  private schedulePendingSurfaceHandoff(surface: ManagedSurface): boolean {
    const requestIds = new Set(
      [...this.pendingSurfaceRequests.values()]
        .filter(
          (pending) =>
            !pending.settled &&
            pending.host === surface.host &&
            pending.surfaceId === surface.surfaceId
        )
        .map((pending) => pending.requestId)
    )
    if (requestIds.size === 0) return false

    const previous = this.pendingSurfaceHandoffs.get(surface.surfaceId)
    if (previous) clearTimeout(previous.timer)
    const timer = setTimeout(
      () => {
        const current = this.pendingSurfaceHandoffs.get(surface.surfaceId)
        if (
          !current ||
          current.generation !== surface.generation ||
          current.host !== surface.host ||
          current.phase !== 'waiting'
        ) {
          return
        }
        const replacement = this.surfaces.get(surface.surfaceId)
        if (replacement && replacement.host === surface.host && !replacement.guest.isDestroyed()) {
          this.pendingSurfaceHandoffs.delete(surface.surfaceId)
          return
        }
        const validRequestIds = [...current.requestIds].filter((requestId) => {
          const pending = this.pendingSurfaceRequests.get(requestId)
          return (
            pending !== undefined &&
            !pending.settled &&
            pending.host === current.host &&
            pending.surfaceId === surface.surfaceId
          )
        })
        if (validRequestIds.length === 0) {
          this.pendingSurfaceHandoffs.delete(surface.surfaceId)
          return
        }
        // Keep the exact handoff record authoritative while rejecting its request releases the
        // Renderer FIFO. A replacement registering between this timer and actual command delivery
        // cancels the record, so the queued generationless close cannot act on that replacement.
        current.phase = 'expired'
        for (const requestId of validRequestIds) {
          this.rejectPendingSurfaceRequest(requestId, 'browser.target_closed', 'target_closed')
        }
        void this.enqueueRendererCommand(async () => {
          const authoritative = this.pendingSurfaceHandoffs.get(surface.surfaceId)
          if (
            authoritative !== current ||
            authoritative.phase !== 'expired' ||
            authoritative.generation !== surface.generation ||
            authoritative.host !== surface.host
          ) {
            return
          }
          if (this.disposed || surface.host.isDestroyed() || this.surfaces.has(surface.surfaceId)) {
            this.pendingSurfaceHandoffs.delete(surface.surfaceId)
            return
          }
          try {
            this.sendCommand(surface.host, {
              schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
              kind: 'closeSurface',
              requestId: randomUUID(),
              surfaceInstanceId: surface.surfaceInstanceId,
              surfaceId: surface.surfaceId
            })
            await nextEventLoopTurn()
          } finally {
            if (this.pendingSurfaceHandoffs.get(surface.surfaceId) === current) {
              this.pendingSurfaceHandoffs.delete(surface.surfaceId)
            }
          }
        }).catch(() => undefined)
      },
      Math.min(STRICT_MODE_SURFACE_HANDOFF_MS, this.attachTimeoutMs)
    )
    this.pendingSurfaceHandoffs.set(surface.surfaceId, {
      generation: surface.generation,
      host: surface.host,
      phase: 'waiting',
      requestIds,
      timer
    })
    return true
  }

  private cancelPendingSurfaceHandoff(surfaceId: string, host: WebContents): void {
    const handoff = this.pendingSurfaceHandoffs.get(surfaceId)
    if (!handoff || handoff.host !== host) return
    clearTimeout(handoff.timer)
    this.pendingSurfaceHandoffs.delete(surfaceId)
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

  private clearActiveSelectionForRetiredSurface(surfaceId: string): void {
    if (this.activeSurfaceId === surfaceId) {
      this.activeSurfaceId = undefined
      this.manualSelectionRevision += 1
    }
    if (this.automationSurfaceId === surfaceId) this.automationSurfaceId = undefined
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

  private handleSurfaceDidStartNavigation(input: {
    event: Event
    generation: number
    guest: WebContents
    isInPlace: boolean
    isMainFrame: boolean
    surfaceId: string
    url: string
  }): void {
    const surface = this.resolveEventSurface(input.surfaceId, input.generation, input.guest)
    if (!surface || !input.isMainFrame || input.isInPlace) return
    const internalDocument = surface.internalDocuments.get(input.url)
    if (internalDocument?.generation === surface.generation) {
      const isCurrentLoad = surface.internalPageLoad?.document === internalDocument
      if (!isCurrentLoad) {
        this.cancelInternalPageLoad(surface)
        surface.navigationEpoch += 1
        surface.navigationUrls.clear()
      }
      surface.navigationInProgress = true
      this.publishSurfaceState(surface)
      return
    }

    const logicalUrl = safeLogicalSurfaceUrl(input.url)
    if (!logicalUrl && !isManagedBlankSurfaceUrl(input.url)) {
      try {
        input.event.preventDefault()
      } catch {
        // did-start-navigation is not cancellable on every Electron release; NetworkGuard also
        // denies this request before commit.
      }
      try {
        input.guest.stop()
      } catch {
        // The strict protocol and session guard already fail closed if the guest disappears.
      }
      return
    }

    const wasPrepared = logicalUrl !== null && surface.preparedNavigationUrl === logicalUrl
    const currentInternalDocument = surface.internalDocuments.get(surface.guest.getURL())
    if (logicalUrl && currentInternalDocument?.logicalUrl === logicalUrl) {
      this.rememberReplaceableHistoryEntry(surface, currentInternalDocument.internalPageUrl)
    }
    surface.preparedNavigationUrl = null
    this.cancelInternalPageLoad(surface)
    if (!wasPrepared) surface.navigationEpoch += 1
    surface.navigationInProgress = true
    surface.pendingNavigationUrl = logicalUrl
    surface.navigationUrls = new Set(logicalUrl ? [logicalUrl] : [])
    surface.presentation = 'content'
    surface.loadError = undefined
    surface.crashError = undefined
    if (logicalUrl) {
      const sameOrigin = haveSameHttpOrigin(surface.logicalUrl, logicalUrl)
      if (!sameOrigin) surface.logicalFaviconUrl = null
      surface.logicalUrl = logicalUrl
      if (!sameOrigin || !surface.logicalTitle)
        surface.logicalTitle = fallbackSurfaceTitle(logicalUrl)
    } else {
      surface.logicalUrl = null
      surface.logicalTitle = null
      surface.logicalFaviconUrl = null
    }
    this.publishSurfaceState(surface)
  }

  private handleSurfaceDidRedirectNavigation(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    url: string
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    const logicalUrl = safeLogicalSurfaceUrl(url)
    if (!surface || !logicalUrl || surface.internalPageLoad) return
    const sameOrigin = haveSameHttpOrigin(surface.logicalUrl, logicalUrl)
    surface.navigationUrls.add(logicalUrl)
    surface.pendingNavigationUrl = logicalUrl
    surface.logicalUrl = logicalUrl
    if (!sameOrigin) {
      surface.logicalFaviconUrl = null
      surface.logicalTitle = fallbackSurfaceTitle(logicalUrl)
    }
    this.publishSurfaceState(surface)
  }

  private handleSurfaceDidFailLoad(input: {
    errorCode: number
    errorDescription: string
    generation: number
    guest: WebContents
    isMainFrame: boolean
    surfaceId: string
    validatedURL: string
  }): void {
    const surface = this.resolveEventSurface(input.surfaceId, input.generation, input.guest)
    if (!surface || !input.isMainFrame) return
    if (isIgnoredBrowserLoadFailure(input.errorCode, input.errorDescription)) return
    const internalDocument = surface.internalDocuments.get(input.validatedURL)
    if (internalDocument) {
      const internalLoad = surface.internalPageLoad
      if (internalLoad?.document === internalDocument) {
        this.finishInternalPageLoad(surface, internalLoad)
      }
      surface.navigationInProgress = false
      this.activateInternalDocument(surface, internalDocument)
      surface.presentation = 'host-fallback'
      this.publishSurfaceState(surface)
      return
    }

    const validatedUrl = safeLogicalSurfaceUrl(input.validatedURL)
    const failedUrl = validatedUrl ?? surface.pendingNavigationUrl ?? surface.logicalUrl
    if (!failedUrl) {
      surface.navigationInProgress = false
      this.publishSurfaceState(surface)
      return
    }
    if (validatedUrl && !surface.navigationUrls.has(validatedUrl)) {
      return
    }
    if (
      surface.loadError?.generation === surface.generation &&
      surface.loadError.navigationEpoch === surface.navigationEpoch &&
      surface.loadError.failedUrl === failedUrl
    ) {
      return
    }

    surface.navigationInProgress = false
    surface.pendingNavigationUrl = failedUrl
    surface.navigationUrls.add(failedUrl)
    surface.logicalUrl = failedUrl
    surface.logicalFaviconUrl = null
    let loadError: BrowserSurfaceLoadError
    try {
      loadError = createBrowserSurfaceLoadError({
        errorCode: input.errorCode,
        errorDescription: input.errorDescription,
        failedUrl,
        generation: surface.generation,
        locale: this.getLocale(),
        navigationEpoch: surface.navigationEpoch
      })
    } catch {
      surface.presentation = 'host-fallback'
      this.publishSurfaceState(surface)
      return
    }
    surface.loadError = loadError
    surface.crashError = undefined
    surface.logicalTitle = loadError.title
    surface.presentation = 'host-fallback'
    this.publishSurfaceState(surface)
    this.rememberReplaceableHistoryEntry(surface, failedUrl)
    try {
      this.beginInternalPageLoad(surface, this.registerInternalLoadError(surface, loadError))
    } catch {
      surface.presentation = 'host-fallback'
      this.publishSurfaceState(surface)
    }
  }

  private handleSurfaceDidNavigate(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    url: string
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (!surface) return
    const internalDocument = surface.internalDocuments.get(url)
    if (internalDocument) {
      const internalLoad = surface.internalPageLoad
      if (internalLoad?.document === internalDocument) {
        this.finishInternalPageLoad(surface, internalLoad)
      }
      surface.navigationInProgress = false
      this.activateInternalDocument(surface, internalDocument)
      this.settlePendingHistoryRemoval(surface)
      this.pruneInternalDocuments(surface)
      this.publishSurfaceState(surface)
      return
    }

    const logicalUrl = safeLogicalSurfaceUrl(url)
    if (!logicalUrl && !isManagedBlankSurfaceUrl(url)) return
    this.cancelInternalPageLoad(surface)
    surface.navigationInProgress = false
    surface.preparedNavigationUrl = null
    surface.pendingNavigationUrl = logicalUrl
    surface.navigationUrls = new Set(logicalUrl ? [logicalUrl] : [])
    surface.presentation = 'content'
    surface.loadError = undefined
    surface.crashError = undefined
    if (logicalUrl) {
      if (!haveSameHttpOrigin(surface.logicalUrl, logicalUrl)) surface.logicalFaviconUrl = null
      surface.logicalUrl = logicalUrl
      surface.logicalTitle = safeSurfaceTitle(guest.getTitle())
    } else {
      surface.logicalUrl = null
      surface.logicalTitle = null
      surface.logicalFaviconUrl = null
    }
    this.settlePendingHistoryRemoval(surface)
    this.pruneInternalDocuments(surface)
    this.publishSurfaceState(surface)
    if (logicalUrl) this.notifyHistoryNavigation(surface)
  }

  private handleSurfaceDidNavigateInPage(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    url: string
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    const logicalUrl = safeLogicalSurfaceUrl(url)
    if (!surface || !logicalUrl || surface.loadError || surface.crashError) return
    surface.logicalUrl = logicalUrl
    surface.pendingNavigationUrl = logicalUrl
    this.publishSurfaceState(surface)
    this.notifyHistoryNavigation(surface)
  }

  private handleSurfaceDidStopLoading(
    surfaceId: string,
    generation: number,
    guest: WebContents
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (!surface || surface.internalPageLoad) return
    surface.navigationInProgress = false
    this.publishSurfaceState(surface)
  }

  private handleSurfaceTitleUpdated(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    title: string
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (surface && this.handleInternalPageAction(surface, title)) return
    if (
      !surface ||
      surface.loadError ||
      surface.crashError ||
      !safeLogicalSurfaceUrl(guest.getURL())
    ) {
      return
    }
    surface.logicalTitle = safeSurfaceTitle(title)
    this.publishSurfaceState(surface)
    this.notifyHistoryMetadata(surface)
  }

  private handleSurfaceFaviconUpdated(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    favicons: string[]
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (
      !surface ||
      surface.loadError ||
      surface.crashError ||
      !safeLogicalSurfaceUrl(guest.getURL())
    ) {
      return
    }
    surface.logicalFaviconUrl = favicons.map(safeRemoteResourceUrl).find(Boolean) ?? null
    this.publishSurfaceState(surface)
    this.notifyHistoryMetadata(surface)
  }

  private notifyHistoryNavigation(surface: ManagedSurface): void {
    const event = this.historyEvent(surface)
    if (!event) return
    try {
      this.onHistoryNavigation?.(event)
    } catch {
      // Browser history is observational and never participates in navigation.
    }
  }

  private notifyHistoryMetadata(surface: ManagedSurface): void {
    const event = this.historyEvent(surface)
    if (!event) return
    try {
      this.onHistoryMetadata?.(event)
    } catch {
      // Browser history is observational and never participates in page metadata updates.
    }
  }

  private historyEvent(surface: ManagedSurface): BrowserSurfaceHistoryEvent | null {
    if (!surface.logicalUrl || surface.loadError || surface.crashError) return null
    return {
      faviconUrl: surface.logicalFaviconUrl,
      generation: surface.generation,
      surfaceId: surface.surfaceId,
      title: surface.logicalTitle,
      url: surface.logicalUrl,
      visitedAt: Date.now()
    }
  }

  private handleSurfaceBeforeInputEvent(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    event: Event,
    keyboardInput: Input
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (!surface || keyboardInput.type !== 'keyDown') return
    const key = keyboardInput.key.toLowerCase()
    const isReload = key === 'f5' || (key === 'r' && (keyboardInput.meta || keyboardInput.control))
    if (!isReload) return
    event.preventDefault()
    this.retrySurface(surface)
  }

  private handleSurfaceRenderProcessGone(
    surfaceId: string,
    generation: number,
    guest: WebContents,
    details: RenderProcessGoneDetails
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (!surface) return
    this.presentRendererFailure(
      surface,
      'renderer_crashed',
      normalizeRendererGoneReason(details.reason)
    )
  }

  private handleSurfaceUnresponsive(
    surfaceId: string,
    generation: number,
    guest: WebContents
  ): void {
    const surface = this.resolveEventSurface(surfaceId, generation, guest)
    if (!surface || surface.crashError?.kind === 'renderer_unresponsive') return
    try {
      guest.stop()
    } catch {
      // The recovery page remains backed by the host fallback if the process cannot be stopped.
    }
    this.presentRendererFailure(surface, 'renderer_unresponsive', 'unresponsive')
  }

  private presentRendererFailure(
    surface: ManagedSurface,
    kind: 'renderer_crashed' | 'renderer_unresponsive',
    reason: string
  ): void {
    this.cancelInternalPageLoad(surface)
    surface.navigationEpoch += 1
    surface.navigationInProgress = false
    surface.navigationUrls.clear()
    surface.preparedNavigationUrl = null
    surface.pendingNavigationUrl = surface.logicalUrl
    surface.logicalFaviconUrl = null
    let crashError: BrowserSurfaceCrashError
    let canPresentInternalPage = true
    try {
      crashError = createBrowserSurfaceCrashError({
        generation: surface.generation,
        kind,
        locale: this.getLocale(),
        logicalUrl: surface.logicalUrl,
        navigationEpoch: surface.navigationEpoch
      })
    } catch {
      canPresentInternalPage = false
      crashError = fallbackBrowserSurfaceCrashError({
        generation: surface.generation,
        kind,
        navigationEpoch: surface.navigationEpoch
      })
    }
    surface.loadError = undefined
    surface.crashError = crashError
    surface.logicalTitle = crashError.title
    surface.presentation = 'host-fallback'
    try {
      this.recordDiagnostic({
        generation: surface.generation,
        kind: kind === 'renderer_crashed' ? 'renderer_process_gone' : 'renderer_unresponsive',
        reason,
        surfaceId: surface.surfaceId
      })
    } catch {
      // Diagnostics are advisory and must never interfere with recovery.
    }
    this.rememberReplaceableHistoryEntry(surface, surface.guest.getURL())
    this.publishSurfaceState(surface)
    if (canPresentInternalPage) {
      try {
        this.beginInternalPageLoad(surface, this.registerInternalCrashError(surface, crashError))
      } catch {
        surface.presentation = 'host-fallback'
        this.publishSurfaceState(surface)
      }
    }
  }

  private retrySurface(surface: ManagedSurface): void {
    const targetUrl = surface.loadError?.failedUrl ?? surface.logicalUrl
    if (surface.loadError || surface.crashError) {
      if (targetUrl)
        this.loadRendererRequestedUrl(surface, targetUrl, { replaceInternalEntry: true })
      return
    }
    this.runGuestNavigationCommand(surface, () => surface.guest.reload())
  }

  private loadRendererRequestedUrl(
    surface: ManagedSurface,
    url: string,
    options: { replaceInternalEntry?: boolean } = {}
  ): void {
    const targetUrl = safeLogicalSurfaceUrl(url)
    if (!targetUrl) throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    if (options.replaceInternalEntry) {
      this.rememberReplaceableHistoryEntry(surface, surface.guest.getURL())
    }
    this.cancelInternalPageLoad(surface)
    surface.navigationEpoch += 1
    const navigationEpoch = surface.navigationEpoch
    surface.navigationInProgress = true
    surface.preparedNavigationUrl = targetUrl
    surface.pendingNavigationUrl = targetUrl
    surface.navigationUrls = new Set([targetUrl])
    surface.presentation = 'content'
    surface.loadError = undefined
    surface.crashError = undefined
    if (!haveSameHttpOrigin(surface.logicalUrl, targetUrl)) surface.logicalFaviconUrl = null
    surface.logicalUrl = targetUrl
    surface.logicalTitle = fallbackSurfaceTitle(targetUrl)
    this.publishSurfaceState(surface)
    this.attemptLogicalNavigation(surface, targetUrl, navigationEpoch, 0)
  }

  private attemptLogicalNavigation(
    surface: ManagedSurface,
    targetUrl: string,
    navigationEpoch: number,
    attempt: number
  ): void {
    if (!this.isCurrentLogicalNavigation(surface, targetUrl, navigationEpoch)) return
    try {
      void surface.guest.loadURL(targetUrl).catch((error: unknown) => {
        if (
          isNavigationAlreadyPendingError(error) &&
          attempt + 1 < INTERNAL_ERROR_PAGE_MAX_ATTEMPTS &&
          this.isCurrentLogicalNavigation(surface, targetUrl, navigationEpoch)
        ) {
          setTimeout(
            () => this.attemptLogicalNavigation(surface, targetUrl, navigationEpoch, attempt + 1),
            INTERNAL_ERROR_PAGE_RETRY_DELAY_MS
          )
          return
        }
        this.handleNavigationPromiseRejection(surface, targetUrl, navigationEpoch, error)
      })
    } catch (error) {
      this.handleNavigationPromiseRejection(surface, targetUrl, navigationEpoch, error)
    }
  }

  private isCurrentLogicalNavigation(
    surface: ManagedSurface,
    targetUrl: string,
    navigationEpoch: number
  ): boolean {
    return Boolean(
      this.surfaces.get(surface.surfaceId) === surface &&
      !surface.guest.isDestroyed() &&
      surface.navigationEpoch === navigationEpoch &&
      surface.navigationUrls.has(targetUrl) &&
      !surface.loadError &&
      !surface.crashError
    )
  }

  private runGuestNavigationCommand(surface: ManagedSurface, command: () => void): void {
    try {
      command()
    } catch {
      surface.navigationInProgress = false
      this.publishSurfaceState(surface)
    }
  }

  private handleNavigationPromiseRejection(
    surface: ManagedSurface,
    targetUrl: string,
    navigationEpoch: number,
    error: unknown
  ): void {
    if (!this.isCurrentLogicalNavigation(surface, targetUrl, navigationEpoch)) return
    const description = browserNavigationErrorDescription(error)
    if (isIgnoredBrowserLoadFailure(-2, description)) {
      return
    }
    this.handleSurfaceDidFailLoad({
      errorCode: -2,
      errorDescription: description,
      generation: surface.generation,
      guest: surface.guest,
      isMainFrame: true,
      surfaceId: surface.surfaceId,
      validatedURL: targetUrl
    })
  }

  private beginInternalPageLoad(surface: ManagedSurface, document: BrowserInternalDocument): void {
    this.internalPages.begin(surface, document)
  }

  private finishInternalPageLoad(surface: ManagedSurface, load: InternalPageLoad): void {
    this.internalPages.finish(surface, load)
  }

  private cancelInternalPageLoad(surface: ManagedSurface): void {
    this.internalPages.cancel(surface)
  }

  private registerInternalLoadError(
    surface: ManagedSurface,
    loadError: BrowserSurfaceLoadError
  ): BrowserInternalDocument {
    return this.internalPages.registerLoadError(surface, loadError)
  }

  private registerInternalCrashError(
    surface: ManagedSurface,
    crashError: BrowserSurfaceCrashError
  ): BrowserInternalDocument {
    return this.internalPages.registerCrashError(surface, crashError)
  }

  private activateInternalDocument(
    surface: ManagedSurface,
    document: BrowserInternalDocument
  ): void {
    this.internalPages.activate(surface, document)
  }

  private rememberReplaceableHistoryEntry(surface: ManagedSurface, expectedUrl: string): void {
    this.internalPages.rememberReplaceableHistoryEntry(surface, expectedUrl)
  }

  private settlePendingHistoryRemoval(surface: ManagedSurface): void {
    this.internalPages.settlePendingHistoryRemoval(surface)
  }

  private pruneInternalDocuments(surface: ManagedSurface): void {
    this.internalPages.prune(surface)
  }

  private forgetInternalDocument(surface: ManagedSurface, url: string): void {
    this.internalPages.forget(surface, url)
  }

  private resolveEventSurface(
    surfaceId: string,
    generation: number,
    guest: WebContents
  ): ManagedSurface | null {
    const surface = this.surfaces.get(surfaceId)
    return surface && surface.generation === generation && surface.guest === guest ? surface : null
  }

  private resolveExactRendererSurface(
    host: WebContents,
    input: BrowserSurfaceStateInput
  ): ManagedSurface {
    const surface = this.surfaces.get(input.surfaceId)
    if (
      !surface ||
      surface.host !== host ||
      surface.surfaceInstanceId !== input.surfaceInstanceId ||
      surface.guest.isDestroyed()
    ) {
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    return surface
  }

  private toRendererSurfaceState(surface: ManagedSurface): BrowserSurfaceState {
    const loadError: BrowserSurfacePublicLoadError | null = surface.loadError
      ? toPublicBrowserSurfaceLoadError(surface.loadError)
      : null
    const crashError: BrowserSurfacePublicCrashError | null = surface.crashError
      ? toPublicBrowserSurfaceCrashError(surface.crashError)
      : null
    return {
      schemaVersion: BROWSER_SURFACE_SCHEMA_VERSION,
      surfaceId: surface.surfaceId,
      surfaceInstanceId: surface.surfaceInstanceId,
      stateRevision: surface.stateRevision,
      url: surface.logicalUrl,
      title: surface.logicalTitle,
      faviconUrl: surface.logicalFaviconUrl,
      canGoBack: safeGuestHistoryBoolean(surface.guest, 'canGoBack'),
      canGoForward: safeGuestHistoryBoolean(surface.guest, 'canGoForward'),
      isLoading: surface.loadError || surface.crashError ? false : surface.navigationInProgress,
      presentation: surface.presentation,
      loadError,
      crashError
    }
  }

  private publishSurfaceState(surface: ManagedSurface): void {
    if (this.surfaces.get(surface.surfaceId) !== surface || surface.host.isDestroyed()) return
    // Native popups have their own window; publishing an unknown sidebar surface would imply a
    // Renderer webview that does not exist. Model-facing listSurfaces still includes this page.
    if (surface.nativePopup) return
    const current = this.toRendererSurfaceState(surface)
    const stateKey = JSON.stringify({ ...current, stateRevision: 0 })
    if (surface.lastPublishedStateKey === stateKey) return
    surface.lastPublishedStateKey = stateKey
    surface.stateRevision += 1
    try {
      this.sendState(surface.host, this.toRendererSurfaceState(surface))
    } catch {
      // State delivery is advisory. Renderer can recover the current snapshot over IPC.
    }
  }

  private resolveSurfaceSelection(input: { index?: number; surfaceId?: string }): ManagedSurface {
    if (input.surfaceId !== undefined) {
      const surface = this.surfaces.get(input.surfaceId)
      if (
        surface &&
        !surface.guest.isDestroyed() &&
        !this.closingSurfaceIds.has(surface.surfaceId)
      ) {
        return surface
      }
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
      crashError: surface.crashError ? toPublicBrowserSurfaceCrashError(surface.crashError) : null,
      generation: surface.generation,
      index: knownIndex ?? this.orderedSurfaces().indexOf(surface),
      isActive: surface.surfaceId === this.activeSurfaceId,
      loadError: surface.loadError ? toPublicBrowserSurfaceLoadError(surface.loadError) : null,
      presentation: surface.presentation,
      surfaceId: surface.surfaceId,
      title: surface.logicalTitle ?? safeSurfaceTitle(surface.guest.getTitle()),
      url: safeSurfaceUrl(surface.logicalUrl ?? surface.guest.getURL())
    }
  }

  private async switchActiveSurface(surface: ManagedSurface): Promise<void> {
    if (surface.guest.isDestroyed() || this.surfaces.get(surface.surfaceId) !== surface) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    this.activeSurfaceId = surface.surfaceId
    this.automationSurfaceId = surface.surfaceId
    this.needsReveal = false
  }

  private async activateSurfaceFromAutomation(surface: ManagedSurface): Promise<void> {
    if (surface.guest.isDestroyed() || this.surfaces.get(surface.surfaceId) !== surface) {
      throw new BrowserSurfaceManagerError('browser.target_closed')
    }
    const lease = this.toolSurfaceLease
    if (
      lease &&
      (lease.surfaceId !== surface.surfaceId || lease.generation !== surface.generation)
    ) {
      // A stale numeric tab index must not redirect an already-bound Tool to another admitted
      // page after a preceding tab closes/reorders. Playwright observes this rejection before it
      // commits Context._currentTab.
      throw new BrowserSurfaceManagerError('browser.surface_unavailable')
    }
    this.automationSurfaceId = surface.surfaceId
    const revision = this.toolSurfaceLease?.selectionRevision ?? this.manualSelectionRevision
    if (this.manualSelectionRevision !== revision) return
    if (this.activeSurfaceId !== surface.surfaceId) {
      const host = this.resolveHost()
      if (!host || host.isDestroyed() || surface.host !== host) {
        throw new BrowserSurfaceManagerError('browser.surface_unavailable')
      }
      if (surface.nativePopup) {
        surface.nativePopup.window.show()
        surface.nativePopup.window.focus()
      } else {
        await this.requestSurface(host, 'selectSurface', surface.surfaceId)
      }
      if (this.manualSelectionRevision !== revision) return
      this.activeSurfaceId = surface.surfaceId
    }
    this.needsReveal = false
  }

  private isActiveAttachmentUsable(
    attachment: ActiveAttachment | undefined
  ): attachment is ActiveAttachment {
    return Boolean(attachment?.browser.isConnected())
  }

  private currentAutomationSurfaceId(): string | undefined {
    return this.toolSurfaceLease?.surfaceId ?? this.automationSurfaceId ?? this.activeSurfaceId
  }

  private removeNavigationListeners(surface: ManagedSurface): void {
    this.cancelInternalPageLoad(surface)
    for (const url of [...surface.internalDocuments.keys()])
      this.forgetInternalDocument(surface, url)
    if (surface.initialDocumentReadyTimer) clearTimeout(surface.initialDocumentReadyTimer)
    surface.initialDocumentReadyTimer = undefined
    surface.releaseInitialDocumentReady()
    surface.guest.removeListener('dom-ready', surface.handleInitialDocumentReady)
    surface.guest.removeListener('did-finish-load', surface.handleInitialDocumentReady)
    surface.guest.removeListener('before-input-event', surface.handleBeforeInputEvent)
    surface.guest.removeListener('did-start-navigation', surface.handleDidStartNavigation)
    surface.guest.removeListener('did-redirect-navigation', surface.handleDidRedirectNavigation)
    surface.guest.removeListener('did-navigate', surface.handleDidNavigate)
    surface.guest.removeListener('did-navigate-in-page', surface.handleDidNavigateInPage)
    surface.guest.removeListener('did-fail-load', surface.handleDidFailLoad)
    surface.guest.removeListener('did-stop-loading', surface.handleDidStopLoading)
    surface.guest.removeListener('page-title-updated', surface.handleTitleUpdated)
    surface.guest.removeListener('page-favicon-updated', surface.handleFaviconUpdated)
    surface.guest.removeListener('render-process-gone', surface.handleRenderProcessGone)
    surface.guest.removeListener('responsive', surface.handleResponsive)
    surface.guest.removeListener('unresponsive', surface.handleUnresponsive)
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
