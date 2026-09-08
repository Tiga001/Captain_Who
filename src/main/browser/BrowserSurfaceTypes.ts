import type { BrowserWindow, Event, Input, RenderProcessGoneDetails, WebContents } from 'electron'
import type { Browser, BrowserContext, ConnectOverCDPTransport } from 'playwright'
import type {
  BrowserSurfaceCommand,
  BrowserSurfacePresentation,
  BrowserSurfacePublicCrashError,
  BrowserSurfacePublicLoadError,
  BrowserSurfaceState
} from '@mycopilot/protocol'
import type { BrowserTargetBroker } from './BrowserTargetBroker'
import type {
  ElectronSurfaceGroupCdpTransport,
  ManagedTargetCreationIntent
} from './ElectronSurfaceGroupCdpTransport'
import type {
  BrowserInternalNavigationLease,
  BrowserNetworkGuard,
  BrowserTargetCreationAuthority
} from './BrowserNetworkGuard'
import type { BrowserInternalPageStoreLike } from './BrowserInternalPageStore'
import type { BrowserSurfaceCrashError, BrowserSurfaceLoadError } from './BrowserLoadErrorPage'

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
  connectOverCdp?: (transport: ConnectOverCDPTransport) => Promise<Browser>
  createSurfaceId?: () => string
  getLocale?: () => string
  internalPageStore: BrowserInternalPageStoreLike
  onHistoryMetadata?: (event: BrowserSurfaceHistoryEvent) => void
  onHistoryNavigation?: (event: BrowserSurfaceHistoryEvent) => void
  releaseSurfaceResources?: (input: { surfaceId: string; generation: number }) => Promise<void>
  resolveHost: () => WebContents | null
  sendCommand: (host: WebContents, command: BrowserSurfaceCommand) => void
  sendState?: (host: WebContents, state: BrowserSurfaceState) => void
  maxSurfaces?: number
  recordDiagnostic?: (diagnostic: BrowserSurfaceDiagnostic) => void
}

export interface BrowserSurfaceHistoryEvent {
  faviconUrl: string | null
  generation: number
  surfaceId: string
  title: string | null
  url: string
  visitedAt: number
}

export interface BrowserSurfaceDiagnostic {
  generation: number
  kind: 'renderer_process_gone' | 'renderer_unresponsive'
  reason: string
  surfaceId: string
}

export interface BrowserSurfaceView {
  crashError: BrowserSurfacePublicCrashError | null
  generation: number
  index: number
  isActive: boolean
  loadError: BrowserSurfacePublicLoadError | null
  presentation: BrowserSurfacePresentation
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

export interface BrowserToolSurfaceLease {
  closeSurface(): Promise<void>
  finish(): void
  generation: number
  index: number
  printToPdf(): Promise<Uint8Array>
  resolveIndex(): Promise<number>
  resizeSurface(input: { height: number; width: number }): Promise<{
    height: number
    width: number
  }>
  selectionRevision: number
  surfaceId: string
}

export interface ActiveSensitiveDispatchFence {
  finishSilently(): boolean
}

export interface ManagedSurface {
  nativePopup?: {
    window: BrowserWindow
    openerSurfaceId: string
    openerGeneration: number
    cleanup: () => void
  }
  createdSequence: number
  dispatchFence?: ActiveSensitiveDispatchFence
  generation: number
  guest: WebContents
  handleBeforeInputEvent: (event: Event, input: Input) => void
  handleInitialDocumentReady: () => void
  handleDidFailLoad: (
    event: Event,
    errorCode: number,
    errorDescription: string,
    validatedURL: string,
    isMainFrame: boolean
  ) => void
  handleDidNavigate: (event: Event, url: string) => void
  handleDidRedirectNavigation: (
    event: Event,
    url: string,
    isInPlace: boolean,
    isMainFrame: boolean
  ) => void
  handleDidNavigateInPage: (event: Event, url: string, isMainFrame: boolean) => void
  handleDidStartNavigation: (
    event: Event,
    url: string,
    isInPlace: boolean,
    isMainFrame: boolean
  ) => void
  handleDidStopLoading: () => void
  handleDestroyed: () => void
  handleFaviconUpdated: (event: Event, favicons: string[]) => void
  handleRenderProcessGone: (event: Event, details: RenderProcessGoneDetails) => void
  handleResponsive: () => void
  handleUnresponsive: () => void
  host: WebContents
  initialDocumentReady: boolean
  initialDocumentReadyPromise: Promise<void>
  initialDocumentReadyTimer?: ReturnType<typeof setTimeout>
  internalPageLoad?: InternalPageLoad
  internalDocuments: Map<string, BrowserInternalDocument>
  crashError?: BrowserSurfaceCrashError
  loadError?: BrowserSurfaceLoadError
  logicalFaviconUrl: string | null
  logicalTitle: string | null
  logicalUrl: string | null
  navigationEpoch: number
  navigationInProgress: boolean
  navigationUrls: Set<string>
  pendingHistoryRemovals: PhysicalHistoryEntry[]
  pendingNavigationUrl: string | null
  preparedNavigationUrl: string | null
  presentation: BrowserSurfacePresentation
  releaseInitialDocumentReady: () => void
  /** Opaque Renderer-visible binding for this exact Main-owned generation. */
  surfaceInstanceId: string
  surfaceId: string
  stateRevision: number
  lastPublishedStateKey?: string
  handleTitleUpdated: (event: Event, title: string) => void
}

export interface InternalPageLoad {
  attempt: number
  document: BrowserInternalDocument
  lease?: BrowserInternalNavigationLease
  retryTimer?: ReturnType<typeof setTimeout>
}

export interface PhysicalHistoryEntry {
  index: number
  url: string
}

export interface BrowserInternalDocument {
  actionUrl: string
  createdSequence: number
  crashError?: BrowserSurfaceCrashError
  generation: number
  internalPageUrl: string
  loadError?: BrowserSurfaceLoadError
  logicalUrl: string | null
  navigationEpoch: number
}

export interface ActiveAttachment {
  browser: Browser
  context: BrowserContext
  transport: ElectronSurfaceGroupCdpTransport
}

export interface PendingEnsure {
  activate?: boolean
  host: WebContents
  kind: 'ensureAttached' | 'createSurface' | 'selectSurface' | 'resizeSurface'
  dimensions?: { height: number; width: number }
  promise: Promise<ManagedSurface>
  surfaceId: string
  reject: (error: BrowserSurfaceManagerError) => void
  requestId: string
  /** Main-issued binding echoed by the exact Renderer webview that acknowledged this request. */
  rendererReadySurfaceInstanceId?: string
  resolve: (surface: ManagedSurface) => void
  settled: boolean
  timer: ReturnType<typeof setTimeout>
}

export interface PendingClose {
  generation: number
  reject: (error: BrowserSurfaceManagerError) => void
  resolve: () => void
  timer: ReturnType<typeof setTimeout>
}

export type SettledSurfaceReadyReason =
  'already_ready' | 'request_expired' | 'request_superseded' | 'request_cancelled' | 'target_closed'

export interface SettledSurfaceRequest {
  dimensions?: { height: number; width: number }
  host: WebContents
  kind: PendingEnsure['kind']
  reason: SettledSurfaceReadyReason
  requestId: string
  surfaceInstanceId?: string
  surfaceId: string
}

export interface PendingSurfaceHandoff {
  generation: number
  host: WebContents
  phase: 'waiting' | 'expired'
  requestIds: ReadonlySet<string>
  timer: ReturnType<typeof setTimeout>
}

export interface PendingSurfaceGroupAdmission {
  attachment: ActiveAttachment
  attempt: Promise<void>
  deferred: boolean
  release(): void
  released: boolean
}

export interface ActiveTargetCreationIntent {
  authority?: BrowserTargetCreationAuthority
  finished: boolean
  intent: ManagedTargetCreationIntent
  transportFinish?: () => void
}
