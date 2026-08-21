import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import type { BrowserContext, ElementHandle, Frame, Locator, Page, Route } from 'playwright'
import type { BrowserArtifactKind, BrowserArtifactReference } from '@mycopilot/protocol'
import { createConnection } from '@playwright/mcp'
import { chmod, copyFile, mkdir, mkdtemp, rm, stat, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, dirname, extname, join } from 'node:path'
import type {
  BrowserNetworkOperationLease,
  BrowserTargetCreationAuthority
} from '../browser/BrowserNetworkGuard'
import {
  BrowserArtifactBrokerError,
  type BrowserArtifactBroker,
  BrowserArtifactOutputSession,
  BrowserArtifactOwner,
  BrowserArtifactReservation
} from '../browser/BrowserArtifactBroker'
import { BrowserDownloadBrokerError } from '../browser/BrowserDownloadBroker'
import {
  BrowserFileBrokerError,
  type BrowserFileBroker,
  type BrowserFileOwner,
  type BrowserFileReadLease,
  type BrowserFileRetainedLease
} from '../browser/BrowserFileBroker'
import {
  formatBrowserFrameEditorCandidates,
  probeBrowserFrameEditors,
  type BrowserFrameEditorCandidate,
  type BrowserFrameEditorKind
} from '../browser/BrowserFrameEditorProbe'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'
import { matchesBoundedJsonSchema } from './boundedJsonSchema'
import {
  MANAGED_PLAYWRIGHT_CAPABILITIES,
  validateAndIndexOfficialPlaywrightCatalog
} from './managedPlaywrightCatalog'
import {
  ManagedPlaywrightSensitiveGrantError,
  sensitiveBindingScopeForInvocation,
  sensitivePolicyForTool,
  sensitiveToolNeedsGrant,
  validateSensitiveToolGrant,
  type ManagedPlaywrightSensitiveGrantLease
} from './managedPlaywrightSensitivePolicy'
import {
  ManagedPlaywrightSensitiveTargetBindingError,
  type ManagedPlaywrightSensitiveTargetBindingLease,
  type ManagedPlaywrightSensitiveTargetBindingStore,
  type ManagedPlaywrightSensitiveTargetIdentity
} from './ManagedPlaywrightSensitiveTargetBindingBroker'

import {
  MANAGED_PLAYWRIGHT_MANIFEST,
  MANAGED_PLAYWRIGHT_PACKAGE_VERSION,
  managedPlaywrightTool,
  type ManagedPlaywrightToolManifestEntry
} from './managedPlaywrightManifest'

const DEFAULT_TOOL_TIMEOUT_MS = 60_000
const MAX_TOOL_TIMEOUT_MS = 300_000
const MAX_ACTIVE_CALLS = 8
const MAX_ARGUMENT_BYTES = 64 * 1024
const MAX_ARGUMENT_DEPTH = 32
const MAX_ARGUMENT_NODES = 4_096
const MAX_RESULT_BYTES = 4 * 1024 * 1024
const MAX_CONTENT_BLOCKS = 128
const MAX_TEXT_BYTES = 256 * 1024
const MAX_ENCODED_MEDIA_BYTES = 1024 * 1024
const MAX_TOTAL_ENCODED_MEDIA_BYTES = 2 * 1024 * 1024
const MAX_STRUCTURED_CONTENT_BYTES = 256 * 1024
const MAX_STRUCTURED_STRING_BYTES = 64 * 1024
const MAX_STRUCTURED_PROPERTIES = 256
const MAX_RESOURCE_URI_BYTES = 8 * 1024
const MAX_RESOURCE_NAME_BYTES = 1024
const MAX_RESOURCE_TITLE_BYTES = 4 * 1024
const MAX_RESOURCE_DESCRIPTION_BYTES = 16 * 1024
const MAX_RESOURCE_MIME_TYPE_BYTES = 256
const MAX_RESOURCE_TEXT_BYTES = MAX_TEXT_BYTES
const CONNECTION_CLOSE_SETTLE_MS = 2_000
const MAX_READ_IMAGE_BYTES = 8 * 1024 * 1024
// The fixed browser_drop implementation both base64-encodes file bytes and waits for OOPIF
// actionability through the guest CDP relay. Every approved path therefore uses the exact-page
// Host adapter; pure MIME data remains on the official path.
const PDF_UNAVAILABLE_SENTINEL = 'browser.pdf_unavailable'

// Pinned @playwright/mcp 0.0.79 `defineTabTool` inventory. These handlers call ensureTab(), so
// Host first leases the exact visible managed Surface and synchronizes the official current Tab.
const CREATING_PAGE_TOOLS = new Set([
  'browser_annotate',
  'browser_click',
  'browser_cookie_set',
  'browser_console_messages',
  'browser_drag',
  'browser_drop',
  'browser_evaluate',
  'browser_file_upload',
  'browser_fill_form',
  'browser_find',
  'browser_generate_locator',
  'browser_handle_dialog',
  'browser_hide_highlight',
  'browser_highlight',
  'browser_hover',
  'browser_localstorage_clear',
  'browser_localstorage_delete',
  'browser_localstorage_get',
  'browser_localstorage_list',
  'browser_localstorage_set',
  'browser_mouse_click_xy',
  'browser_mouse_down',
  'browser_mouse_drag_xy',
  'browser_mouse_move_xy',
  'browser_mouse_up',
  'browser_mouse_wheel',
  'browser_navigate',
  'browser_navigate_back',
  'browser_network_request',
  'browser_network_requests',
  'browser_pdf_save',
  'browser_press_key',
  'browser_resize',
  'browser_run_code_unsafe',
  'browser_select_option',
  'browser_sessionstorage_clear',
  'browser_sessionstorage_delete',
  'browser_sessionstorage_get',
  'browser_sessionstorage_list',
  'browser_sessionstorage_set',
  'browser_snapshot',
  'browser_take_screenshot',
  'browser_type',
  'browser_verify_element_visible',
  'browser_verify_list_visible',
  'browser_verify_text_visible',
  'browser_verify_value'
])

const EXISTING_PAGE_TOOLS = new Set([
  'browser_video_chapter',
  'browser_video_hide_actions',
  'browser_video_show_actions',
  'browser_wait_for'
])

export type ManagedPlaywrightMcpHostErrorCode =
  | 'mcp.builtin_playwright.closed'
  | 'mcp.builtin_playwright.busy'
  | 'mcp.builtin_playwright.cancelled'
  | 'mcp.builtin_playwright.timeout'
  | 'mcp.builtin_playwright.tool_not_reviewed'
  | 'mcp.builtin_playwright.invalid_arguments'
  | 'mcp.builtin_playwright.sensitive_grant_missing'
  | 'mcp.builtin_playwright.sensitive_grant_drifted'
  | 'mcp.builtin_playwright.sensitive_grant_expired'
  | 'mcp.builtin_playwright.sensitive_grant_origin_drifted'
  | 'mcp.builtin_playwright.sensitive_grant_reused'
  | 'mcp.builtin_playwright.sensitive_target_scope_unsupported'
  | 'mcp.builtin_playwright.sensitive_request_identity_unavailable'
  | 'mcp.builtin_playwright.catalog_drift'
  | 'mcp.builtin_playwright.output_too_large'
  | 'mcp.builtin_playwright.protocol_error'
  | 'browser.surface_unavailable'
  | 'browser.surface_capacity_exceeded'
  | 'browser.target_closed'
  | 'browser.risk_outcome_unknown'

export class ManagedPlaywrightMcpHostError extends Error {
  readonly name = 'ManagedPlaywrightMcpHostError'

  constructor(
    readonly code: ManagedPlaywrightMcpHostErrorCode,
    readonly dispatchCertainty?:
      'definitely_not_dispatched' | 'possibly_dispatched' | 'response_received'
  ) {
    super(code)
  }
}

export interface ManagedPlaywrightToolDescriptor {
  name: string
  description: string
  inputSchema: Record<string, unknown>
  annotations: Record<string, unknown>
}

export interface ManagedPlaywrightCallResult {
  content: ManagedPlaywrightContentBlock[]
  structuredContent?: Record<string, unknown>
  isError: boolean
  /**
   * Main/Core-only absolute screenshot file used to publish an `image-artifact://` readPath.
   * Callers must omit this from the MCP `tools/call` result object.
   */
  hostImagePublishPath?: string
}

export interface ManagedPlaywrightProtocolSnapshot {
  negotiatedVersion: '2025-11-25'
  lifecycle: 'initialize_fallback'
  server: { name: '@playwright/mcp'; version: typeof MANAGED_PLAYWRIGHT_PACKAGE_VERSION }
  capabilities: {
    tools: true
    toolsListChanged: false
    resources: false
    resourcesListChanged: false
    resourcesSubscribe: false
    prompts: false
    promptsListChanged: false
    logging: false
    completions: false
    tasks: false
    extensions: []
  }
}

export type ManagedPlaywrightContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; data: string; mime_type: string }
  | { type: 'audio'; data: string; mime_type: string }
  | {
      type: 'embedded_resource'
      resource:
        | { contentType: 'text'; uri: string; text: string; mime_type?: string }
        | { contentType: 'blob'; uri: string; data: string; mime_type?: string }
    }
  | {
      type: 'resource_link'
      resource: {
        uri: string
        name: string
        title?: string
        description?: string
        mimeType?: string
        size?: number
      }
    }

export interface ManagedPlaywrightMcpHostOptions {
  beginNetworkOperation?: (input: {
    authorizationContext: BrowserRiskAuthorizationContext
    parentRequestId: string
    signal: AbortSignal
  }) => Promise<BrowserNetworkOperationLease>
  beginTargetCreationOperation?: (input: {
    authorizationContext: BrowserRiskAuthorizationContext
    parentRequestId: string
    signal: AbortSignal
  }) => Promise<BrowserNetworkOperationLease>
  getBrowserContext: () => Promise<BrowserContext>
  getActiveSurfaceIdentity?: () => { surfaceId: string; generation: number } | null
  sensitiveTargetBindings?: ManagedPlaywrightSensitiveTargetBindingStore
  artifactBroker?: BrowserArtifactBroker
  fileBroker?: BrowserFileBroker
  finalizeBrowserRun?: (runId: string) => Promise<void>
  releaseBrowserCapability?: (activationId: string) => Promise<void>
  releaseBrowserToolCall?: (input: { runId: string; toolCallId: string }) => Promise<void>
  surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
  closeSurface: () => Promise<void>
  detachAutomation: () => Promise<void>
  toolTimeoutMs?: number
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  createClient?: () => ManagedMcpClient
}

export interface ManagedPlaywrightSurfaceView {
  surfaceId: string
  index: number
  title: string
  url: string
  isActive: boolean
  generation: number
}

export interface ManagedPlaywrightSurfaceGroupAdapter {
  /** Main-only identity; never include this projection in Renderer surface ViewModels. */
  getSensitiveTargetIdentity(): ManagedPlaywrightSensitiveTargetIdentity | null
  ensureActiveSurface(): Promise<ManagedPlaywrightSurfaceView>
  listSurfaces(): readonly ManagedPlaywrightSurfaceView[]
  createSurface(input?: { url?: string }): Promise<ManagedPlaywrightSurfaceView>
  selectSurface(input: { index: number }): Promise<ManagedPlaywrightSurfaceView>
  closeSurfaceByIndex(index?: number): Promise<void>
  /** Freezes the trusted UI-selected surface for one serialized Tool dispatch. */
  beginToolSurfaceLease?(): Promise<ManagedPlaywrightToolSurfaceLease>
  /** Locks the selected existing surface without creating or revealing a tab. */
  beginExistingToolSurfaceLease?(): Promise<ManagedPlaywrightToolSurfaceLease | null>
  /** Locks one exact model-visible tab index without selecting or revealing it. */
  beginToolSurfaceLeaseByIndex?(index: number): Promise<ManagedPlaywrightToolSurfaceLease>
  /** Narrows Browser-level target creation to the reviewed Tool's expected UX. */
  beginTargetCreationIntent?(
    intent: 'background' | 'interactive',
    authority?: BrowserTargetCreationAuthority
  ): () => void
  createInitialTargetSurface?(input: {
    authority: BrowserTargetCreationAuthority
  }): Promise<ManagedPlaywrightSurfaceView>
  resizeActiveSurface?(input: { width: number; height: number }): Promise<{
    width: number
    height: number
  }>
}

export interface ManagedPlaywrightToolSurfaceLease {
  readonly surfaceId: string
  readonly generation: number
  /** Admission-time snapshot only. Dispatch must use resolveIndex(). */
  readonly index: number
  readonly selectionRevision: number
  resolveIndex(): Promise<number>
  closeSurface(): Promise<void>
  resizeSurface?(input: { width: number; height: number }): Promise<{
    width: number
    height: number
  }>
  finish(): void
}

export type ManagedPlaywrightConnectionFactory = (
  config: Parameters<typeof createConnection>[0],
  contextGetter: () => Promise<BrowserContext>
) => Promise<ManagedMcpServer>

interface ManagedMcpServer {
  onclose?: () => void
  connect(transport: InMemoryTransport): Promise<void>
  close(): Promise<void>
}

export interface ManagedMcpClient {
  onclose?: () => void
  connect(transport: InMemoryTransport): Promise<void>
  listTools(
    params?: { cursor?: string },
    options?: { signal?: AbortSignal; timeout?: number }
  ): Promise<{ tools: UpstreamToolDescriptor[]; nextCursor?: string }>
  callTool(
    params: { name: string; arguments: Record<string, unknown> },
    resultSchema?: undefined,
    options?: { signal?: AbortSignal; timeout?: number; resetTimeoutOnProgress?: boolean }
  ): Promise<unknown>
  close(): Promise<void>
}

interface UpstreamToolDescriptor {
  name: string
  description?: string
  inputSchema: Record<string, unknown>
  annotations?: Record<string, unknown>
}

interface ActiveConnection {
  client: ManagedMcpClient
  server: ManagedMcpServer
  clientTransport: InMemoryTransport
  serverTransport: InMemoryTransport
  outputDirectory: string
  outputLifetime: ManagedConnectionOutputLifetime
  outputSession?: BrowserArtifactOutputSession
  catalog?: ReadonlyMap<string, UpstreamToolDescriptor>
  cataloging?: Promise<ReadonlyMap<string, UpstreamToolDescriptor>>
  context?: BrowserContext
  contextCloseHandler?: () => void
  closePromise?: Promise<void>
  retirementPromise?: Promise<void>
  stale: boolean
}

interface ManagedConnectionOutputLifetime {
  retainDirectory(directory: string): () => Promise<void>
  retireConnection(): Promise<void>
}

interface PreparedSensitiveFileLease extends BrowserFileReadLease {
  markDispatched?(): void
}

interface ManagedRouteDefinition {
  addHeaders?: Readonly<Record<string, string>>
  body?: string
  contentType?: string
  pattern: string
  removeHeaders?: readonly string[]
  runId: string
  status?: number
}

interface AppliedContextNetworkState {
  handleClose: () => void
  routes: Map<ManagedRouteDefinition, (route: Route) => Promise<void>>
  offline: boolean
}

interface ManagedTracingState {
  context: BrowserContext
  owner: BrowserArtifactOwner
  reservation: BrowserArtifactReservation
}

interface ManagedUpstreamArtifactPlan {
  hostWritesDiagnosticText: boolean
  reservation: BrowserArtifactReservation
  serverArguments: Record<string, unknown>
}

interface HostAdapterInput {
  name: string
  arguments: Record<string, unknown>
  modelArguments: Record<string, unknown>
  options: {
    authorizationContext?: BrowserRiskAuthorizationContext
    parentRequestId?: string
  }
  connection: ActiveConnection
  surfaceLease?: ManagedPlaywrightToolSurfaceLease
  signal: AbortSignal
  markDispatched(): Promise<void>
}

interface PreparedSensitiveGrant {
  readonly lease: ManagedPlaywrightSensitiveGrantLease
  readonly targetBinding: ManagedPlaywrightSensitiveTargetBindingLease
  readonly target?: ManagedPlaywrightSensitiveTargetIdentity
}

interface RegisteredFrameEditorCandidate {
  readonly activationId?: string
  readonly element: ElementHandle<HTMLElement>
  readonly expiresAtMs: number
  readonly frame: Frame
  readonly generation: number
  readonly kind: BrowserFrameEditorKind
  readonly runId?: string
  readonly surfaceId: string
}

const FRAME_EDITOR_TARGET_PREFIX = 'managed-frame-editor:'
const FRAME_EDITOR_TARGET_TTL_MS = 60_000

/**
 * Main-process, in-memory host for the exact official Playwright MCP package.
 *
 * Discovery never resolves a BrowserContext. The official server receives the context getter and
 * invokes it lazily on the first real browser operation. Every model-visible schema comes from the
 * reviewed Host manifest, and calls outside that manifest are rejected before reaching upstream.
 */
export class ManagedPlaywrightMcpHost {
  private readonly artifactBroker?: BrowserArtifactBroker
  private readonly beginNetworkOperation?: ManagedPlaywrightMcpHostOptions['beginNetworkOperation']
  private readonly beginTargetCreationOperation?: ManagedPlaywrightMcpHostOptions['beginTargetCreationOperation']
  private readonly createClient: () => ManagedMcpClient
  private readonly createOfficialConnection: ManagedPlaywrightConnectionFactory
  private readonly detachAutomation: () => Promise<void>
  private readonly getBrowserContext: () => Promise<BrowserContext>
  private readonly getActiveSurfaceIdentity?: ManagedPlaywrightMcpHostOptions['getActiveSurfaceIdentity']
  private readonly fileBroker?: BrowserFileBroker
  private readonly surfaceGroup?: ManagedPlaywrightSurfaceGroupAdapter
  private readonly sensitiveTargetBindings?: ManagedPlaywrightSensitiveTargetBindingStore
  private readonly finalizeBrowserRun?: ManagedPlaywrightMcpHostOptions['finalizeBrowserRun']
  private readonly releaseBrowserCapability?: ManagedPlaywrightMcpHostOptions['releaseBrowserCapability']
  private readonly releaseBrowserToolCall?: ManagedPlaywrightMcpHostOptions['releaseBrowserToolCall']
  private readonly toolTimeoutMs: number

  private readonly activeCalls = new Set<AbortController>()
  private readonly activationIds = new Set<string>()
  private readonly consumedSensitiveGrantIds = new Set<string>()
  private readonly frameEditorCandidates = new Map<string, RegisteredFrameEditorCandidate>()
  private readonly contextNetworkState = new Map<BrowserContext, AppliedContextNetworkState>()
  private dispatchTail: Promise<void> = Promise.resolve()
  private readonly offlineRuns = new Set<string>()
  private readonly routeDefinitions: ManagedRouteDefinition[] = []
  private readonly outputDirectories = new Set<string>()
  private tracing?: ManagedTracingState
  private connection?: ActiveConnection
  private connecting?: Promise<ActiveConnection>
  private connectingEpoch?: number
  private connectionLifecycleTail: Promise<void> = Promise.resolve()
  private connectionEpoch = 0
  private closed = false
  private stateOwnerRunId?: string

  constructor(options: ManagedPlaywrightMcpHostOptions) {
    this.artifactBroker = options.artifactBroker
    this.beginNetworkOperation = options.beginNetworkOperation
    this.beginTargetCreationOperation = options.beginTargetCreationOperation
    this.getBrowserContext = options.getBrowserContext
    this.getActiveSurfaceIdentity = options.getActiveSurfaceIdentity
    this.fileBroker = options.fileBroker
    this.finalizeBrowserRun = options.finalizeBrowserRun
    this.releaseBrowserCapability = options.releaseBrowserCapability
    this.releaseBrowserToolCall = options.releaseBrowserToolCall
    this.surfaceGroup = options.surfaceGroup
    this.sensitiveTargetBindings = options.sensitiveTargetBindings
    this.detachAutomation = options.detachAutomation
    this.createOfficialConnection = options.createOfficialConnection ?? createConnection
    this.createClient =
      options.createClient ??
      (() =>
        new Client(
          { name: 'mycopilot-managed-playwright-mcp-client', version: '1.0.0' },
          { capabilities: {} }
        ))
    this.toolTimeoutMs = boundedTimeout(options.toolTimeoutMs)
  }

  async connect(signal?: AbortSignal): Promise<void> {
    try {
      await this.runBounded(async (operationSignal) => {
        const connection = await this.ensureConnected()
        await this.ensureOfficialCatalog(connection, operationSignal)
      }, signal)
    } catch (error) {
      await this.disposeConnection(true, true)
      throw error
    }
  }

  protocolSnapshot(): ManagedPlaywrightProtocolSnapshot {
    return {
      negotiatedVersion: '2025-11-25',
      lifecycle: 'initialize_fallback',
      server: { name: '@playwright/mcp', version: MANAGED_PLAYWRIGHT_PACKAGE_VERSION },
      capabilities: {
        tools: true,
        toolsListChanged: false,
        resources: false,
        resourcesListChanged: false,
        resourcesSubscribe: false,
        prompts: false,
        promptsListChanged: false,
        logging: false,
        completions: false,
        tasks: false,
        extensions: []
      }
    }
  }

  /** Releases task-scoped Host overlays without closing any manual Browser surface. */
  async releaseRun(runId: string): Promise<void> {
    if (!runId || runId.length > 512) return
    this.sensitiveTargetBindings?.releaseRun(runId)
    await this.clearFrameEditorCandidatesForRun(runId)
    this.offlineRuns.delete(runId)
    await this.fileBroker?.releaseRun(runId)
    for (let index = this.routeDefinitions.length - 1; index >= 0; index -= 1) {
      if (this.routeDefinitions[index].runId === runId) this.routeDefinitions.splice(index, 1)
    }
    if (this.tracing?.owner.runId === runId) {
      const tracing = this.tracing
      this.tracing = undefined
      await settleWithin(
        Promise.allSettled([tracing.context.tracing.stop(), tracing.reservation.discard()]),
        CONNECTION_CLOSE_SETTLE_MS
      )
    }
    await settleWithin(
      this.reconcileManagedNetworkState().catch(() => undefined),
      CONNECTION_CLOSE_SETTLE_MS
    )
    this.maybeReleaseStateOwner(runId)
    const finalize = this.finalizeBrowserRun?.(runId) ?? this.artifactBroker?.finalizeRun(runId)
    if (finalize) {
      await settleWithin(
        finalize.catch(() => undefined),
        CONNECTION_CLOSE_SETTLE_MS
      )
    }
  }

  async listTools(signal?: AbortSignal): Promise<readonly ManagedPlaywrightToolDescriptor[]> {
    const catalog = await this.runBounded(async (operationSignal) => {
      const connection = await this.ensureConnected()
      return this.ensureOfficialCatalog(connection, operationSignal)
    }, signal)
    return MANAGED_PLAYWRIGHT_MANIFEST.tools.map((tool) => {
      const upstream = catalog.get(tool.rawName)
      if (!upstream?.annotations) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
      }
      return {
        name: tool.rawName,
        description: tool.description,
        inputSchema: structuredClone(tool.inputSchema),
        annotations: structuredClone(upstream.annotations)
      }
    })
  }

  async callTool(
    name: string,
    argumentsValue: unknown,
    options: {
      signal?: AbortSignal
      timeoutMs?: number
      authorizationContext?: BrowserRiskAuthorizationContext
      parentRequestId?: string
      onDispatchPhase?: (phase: 'possibly_dispatched' | 'response_received') => Promise<boolean>
    } = {}
  ): Promise<ManagedPlaywrightCallResult> {
    const reviewed = managedPlaywrightTool(name)
    if (!reviewed) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.tool_not_reviewed')
    }
    const modelArguments = expectArgumentRecord(argumentsValue)
    validateReviewedArguments(reviewed, modelArguments)
    if (options.authorizationContext?.activationId) {
      this.activationIds.add(options.authorizationContext.activationId)
    }
    this.assertRunStateAccess(options.authorizationContext?.runId)
    let serverArguments = stripHostArguments(modelArguments)

    let dispatchStarted = false
    let responseReceived = false
    let acknowledgedDispatch = false
    let acknowledgedResponse = false
    const acknowledgeDispatchPhase = async (
      phase: 'possibly_dispatched' | 'response_received'
    ): Promise<void> => {
      if (phase === 'possibly_dispatched' && acknowledgedDispatch) return
      if (phase === 'response_received' && acknowledgedResponse) return
      const accepted = (await options.onDispatchPhase?.(phase)) ?? true
      if (!accepted) {
        throw new ManagedPlaywrightMcpHostError(
          'mcp.builtin_playwright.protocol_error',
          phase === 'response_received' ? 'response_received' : 'definitely_not_dispatched'
        )
      }
      if (phase === 'possibly_dispatched') acknowledgedDispatch = true
      else acknowledgedResponse = true
    }
    try {
      return await this.runBounded(
        async (operationSignal) =>
          this.serializeDispatch(operationSignal, async () => {
            this.assertRunStateAccess(options.authorizationContext?.runId)
            let sensitiveGrant: PreparedSensitiveGrant | undefined
            let fileLease: PreparedSensitiveFileLease | undefined
            let surfaceLease: ManagedPlaywrightToolSurfaceLease | undefined
            let finishTargetCreationIntent: (() => void) | undefined
            const outcome = await (async (): Promise<ManagedPlaywrightCallResult> => {
              // Approval revalidation is a pure pre-dispatch gate. A missing, expired, or drifted
              // grant must not attach automation, create a Surface, or start the managed MCP.
              sensitiveGrant = await this.prepareSensitiveGrant(
                reviewed,
                modelArguments,
                options.authorizationContext
              )
              if (name === 'browser_get_config') {
                responseReceived = true
                return managedBrowserConfigResult()
              }
              if (name === 'browser_close') {
                // The visible surfaces are jointly owned by the user. `browser_close` retires
                // only this automation generation, matching a shared official BrowserContext;
                // browser_tabs close remains the sole tab-closing operation.
                await acknowledgeDispatchPhase('possibly_dispatched')
                dispatchStarted = true
                await this.cleanupManagedState()
                await this.disposeConnection(true, false, true)
                responseReceived = true
                return textToolResult('The managed browser automation context was closed.')
              }
              if (
                name === 'browser_tabs' &&
                modelArguments.action === 'new' &&
                this.beginTargetCreationOperation &&
                this.surfaceGroup?.beginTargetCreationIntent
              ) {
                const result = await this.executeManagedTargetCreatingTool({
                  arguments: serverArguments,
                  options,
                  signal: operationSignal,
                  markDispatched: async () => {
                    await acknowledgeDispatchPhase('possibly_dispatched')
                    dispatchStarted = true
                  },
                  markResponseReceived: () => {
                    responseReceived = true
                  }
                })
                return result
              }
              surfaceLease = await this.acquireToolSurfaceLease(name, modelArguments)
              if (
                name === 'browser_navigate' &&
                !surfaceLease &&
                this.beginTargetCreationOperation &&
                this.surfaceGroup?.beginTargetCreationIntent
              ) {
                // Fixed Playwright implicitly creates its first Page for browser_navigate. That
                // Target.createTarget must receive the same one-shot Main authority as an
                // explicit browser_tabs new; otherwise the SurfaceGroup correctly rejects the
                // creation after dispatch and the user is left on an inert blank tab.
                return await this.executeManagedTargetCreatingTool({
                  toolName: 'browser_navigate',
                  arguments: serverArguments,
                  options,
                  signal: operationSignal,
                  markDispatched: async () => {
                    await acknowledgeDispatchPhase('possibly_dispatched')
                    dispatchStarted = true
                  },
                  markResponseReceived: () => {
                    responseReceived = true
                  }
                })
              }
              if (
                name === 'browser_tabs' &&
                modelArguments.action === 'list' &&
                !surfaceLease &&
                this.beginTargetCreationOperation &&
                this.surfaceGroup?.beginTargetCreationIntent
              ) {
                // The last visible tab may close between the model-visible list snapshot and the
                // optional exact lease. Fixed browser_tabs list then creates one blank page; give
                // that Target.createTarget the same one-shot authority as a zero-group call.
                return await this.executeManagedTargetCreatingTool({
                  arguments: serverArguments,
                  options,
                  signal: operationSignal,
                  markDispatched: async () => {
                    await acknowledgeDispatchPhase('possibly_dispatched')
                    dispatchStarted = true
                  },
                  markResponseReceived: () => {
                    responseReceived = true
                  }
                })
              }
              if (
                sensitiveGrant?.target &&
                this.surfaceGroup?.beginToolSurfaceLease &&
                (!surfaceLease ||
                  surfaceLease.surfaceId !== sensitiveGrant.target.surfaceId ||
                  surfaceLease.generation !== sensitiveGrant.target.generation)
              ) {
                throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
              }
              const targetCreationIntent = targetCreationIntentForTool(
                name,
                modelArguments,
                Boolean(surfaceLease)
              )
              if (targetCreationIntent && this.surfaceGroup?.beginTargetCreationIntent) {
                finishTargetCreationIntent =
                  this.surfaceGroup.beginTargetCreationIntent(targetCreationIntent)
              }
              const connection = await this.ensureConnected()
              await this.ensureOfficialCatalog(connection, operationSignal)
              if (surfaceLease && shouldSynchronizeOfficialSurface(name, modelArguments)) {
                // Pure pre-dispatch CAS: reject an already-closed/replaced lease before lazy
                // Context hydration crosses the official dispatch boundary. Re-resolve again
                // immediately before exact selection because hydration itself can observe close /
                // reorder events from the retained Electron group.
                await surfaceLease.resolveIndex()
              }
              if (
                surfaceLease &&
                name === 'browser_tabs' &&
                (modelArguments.action === 'select' || modelArguments.action === 'close')
              ) {
                serverArguments = {
                  ...serverArguments,
                  index: await surfaceLease.resolveIndex()
                }
              }
              const preparedFiles = await this.prepareSensitiveFiles({
                name,
                arguments: serverArguments,
                authorizationContext: options.authorizationContext,
                connection,
                preparedHandles: sensitiveGrant?.targetBinding.preparedFileHandles
              })
              serverArguments = preparedFiles.arguments
              fileLease = preparedFiles.lease
              let toolDispatchMarked = false
              const markDispatched = async (): Promise<void> => {
                if (toolDispatchMarked) return
                if (sensitiveGrant) {
                  if (sensitiveGrant.target) {
                    this.assertSensitiveDispatchTarget(sensitiveGrant.target)
                  }
                }
                await acknowledgeDispatchPhase('possibly_dispatched')
                dispatchStarted = true
                fileLease?.markDispatched?.()
                if (sensitiveGrant) {
                  sensitiveGrant.targetBinding.markDispatched()
                  sensitiveGrant.lease.markDispatched()
                }
                toolDispatchMarked = true
              }
              let riskLease: BrowserNetworkOperationLease | undefined
              let riskDispatchMarked = false
              const markOperationDispatched = async (): Promise<void> => {
                if (!riskDispatchMarked) {
                  await markDispatched()
                  riskLease?.markDispatched()
                  riskDispatchMarked = true
                  return
                }
                await markDispatched()
              }
              let artifactPlan: ManagedUpstreamArtifactPlan | undefined
              let artifactCommitted = false
              try {
                if (
                  this.beginNetworkOperation &&
                  toolUsesManagedPageNetworkOperation(
                    name,
                    modelArguments,
                    Boolean(surfaceLease),
                    Boolean(this.surfaceGroup?.beginToolSurfaceLease)
                  )
                ) {
                  if (!options.authorizationContext || !options.parentRequestId) {
                    throw new ManagedPlaywrightMcpHostError(
                      'mcp.builtin_playwright.invalid_arguments'
                    )
                  }
                  riskLease = await this.beginNetworkOperation({
                    authorizationContext: options.authorizationContext,
                    parentRequestId: options.parentRequestId,
                    signal: operationSignal
                  })
                  if (name === 'browser_navigate') {
                    const url = serverArguments.url
                    if (typeof url !== 'string') {
                      throw new ManagedPlaywrightMcpHostError(
                        'mcp.builtin_playwright.invalid_arguments'
                      )
                    }
                    try {
                      await riskLease.preflight(url)
                    } catch {
                      return riskFailureResult(riskLease.failure())
                    }
                  }
                }

                if (name === 'browser_tabs' && modelArguments.action === 'close' && surfaceLease) {
                  // Hydration is the first official dispatch for a retained page. Install close
                  // intent before it so a target close racing Context import is attributed to the
                  // reviewed close operation rather than reported as an unrelated risk failure.
                  riskLease?.expectTargetClose({
                    surfaceId: surfaceLease.surfaceId,
                    generation: surfaceLease.generation
                  })
                }

                if (surfaceLease && !connection.context) {
                  // A fresh official MCP connection starts with an empty in-memory tab registry,
                  // even when Electron kept the user's Page alive across browser_close / task
                  // release. Official browser_tabs list is the lazy Context hydration boundary:
                  // with an exact retained lease it imports browserContext.pages() and cannot
                  // create a target. Do this before select/close/list or any Page Tool touches the
                  // official currentTab. A true zero-page call has no lease and deliberately skips
                  // this path so fixed Playwright retains its reviewed first-target semantics.
                  await this.hydrateOfficialRetainedContext(
                    connection,
                    operationSignal,
                    markOperationDispatched,
                    async () => {
                      responseReceived = true
                      await acknowledgeDispatchPhase('response_received')
                    }
                  )
                }

                if (
                  surfaceLease &&
                  name === 'browser_tabs' &&
                  (modelArguments.action === 'select' || modelArguments.action === 'close')
                ) {
                  serverArguments = {
                    ...serverArguments,
                    index: await surfaceLease.resolveIndex()
                  }
                }

                if (surfaceLease && shouldSynchronizeOfficialSurface(name, modelArguments)) {
                  // Bringing the exact managed guest to the front can synchronously run a page
                  // focus handler that navigates or downloads. Install the exact guest's risk /
                  // download authority and conservatively cross the dispatch boundary first. The
                  // early CAS above kept pre-existing drift definite; this second resolution keeps
                  // retained-context hydration from redirecting selection after a close/reorder.
                  const exactSurfaceIndex = await surfaceLease.resolveIndex()
                  await markOperationDispatched()
                  await this.selectOfficialSurface(
                    connection,
                    exactSurfaceIndex,
                    operationSignal,
                    async () => {
                      responseReceived = true
                      await acknowledgeDispatchPhase('response_received')
                    }
                  )
                }

                let hostAdapted: ManagedPlaywrightCallResult | undefined
                try {
                  hostAdapted = await this.executeHostAdapter({
                    name,
                    arguments: serverArguments,
                    modelArguments,
                    options,
                    connection,
                    surfaceLease,
                    signal: operationSignal,
                    markDispatched: markOperationDispatched
                  })
                } catch (error) {
                  try {
                    await riskLease?.settle()
                  } catch {
                    const pendingFailure = riskLease?.failure()
                    if (pendingFailure) return riskFailureResult(pendingFailure)
                  }
                  const failure = riskLease?.failure()
                  if (failure) return riskFailureResult(failure)
                  throw error
                }
                if (hostAdapted) {
                  try {
                    await riskLease?.settle()
                  } catch {
                    const pendingFailure = riskLease?.failure()
                    if (pendingFailure) return riskFailureResult(pendingFailure)
                    throw new ManagedPlaywrightMcpHostError(
                      'browser.risk_outcome_unknown',
                      dispatchStarted ? 'possibly_dispatched' : 'definitely_not_dispatched'
                    )
                  }
                  const failure = riskLease?.failure()
                  if (failure) return riskFailureResult(failure)
                  responseReceived = true
                  return withArtifactReferences(hostAdapted, riskLease?.artifacts() ?? [])
                }

                let officialResult: ManagedPlaywrightCallResult
                try {
                  artifactPlan = await this.prepareUpstreamArtifactPlan({
                    name,
                    arguments: serverArguments,
                    modelArguments,
                    options,
                    connection,
                    surfaceLease
                  })
                  // Once the official MCP handler receives the call, page script, navigation, or form
                  // submission may already have happened. Later network refusals are therefore never
                  // represented as a safe pre-dispatch denial and must not be replayed automatically.
                  await markOperationDispatched()
                  officialResult = await this.callOfficialTool(
                    connection,
                    { name, arguments: artifactPlan?.serverArguments ?? serverArguments },
                    operationSignal,
                    boundedTimeout(options.timeoutMs ?? this.toolTimeoutMs),
                    async () => {
                      // The official peer has produced an authoritative raw response. Record and
                      // acknowledge that boundary before bounded parsing: a locally rejected
                      // oversized/malformed response is still response_received, not a transport
                      // ambiguity, and must not cause this healthy connection to be retired.
                      responseReceived = true
                      await acknowledgeDispatchPhase('response_received')
                    }
                  )
                } catch (error) {
                  await artifactPlan?.reservation.discard().catch(() => undefined)
                  try {
                    await riskLease?.settle()
                  } catch {
                    const pendingFailure = riskLease?.failure()
                    if (pendingFailure) return riskFailureResult(pendingFailure)
                  }
                  const failure = riskLease?.failure()
                  if (failure) return riskFailureResult(failure)
                  throw error
                }
                try {
                  await riskLease?.settle()
                } catch (error) {
                  const pendingFailure = riskLease?.failure()
                  if (pendingFailure) return riskFailureResult(pendingFailure)
                  throw error
                }
                const failure = riskLease?.failure()
                if (failure) {
                  await artifactPlan?.reservation.discard().catch(() => undefined)
                  return riskFailureResult(failure)
                }
                if (operationSignal.aborted) throw cancellationError(operationSignal.reason)
                let parsed = adaptReviewedToolResult(name, officialResult)
                if (name === 'browser_snapshot' && !artifactPlan && !parsed.isError) {
                  try {
                    parsed = await this.appendSafeFrameEditorCandidates(
                      parsed,
                      await this.getExactManagedPage(surfaceLease),
                      options.authorizationContext
                    )
                  } catch {
                    // Candidate discovery is an optional, value-free fallback. A target close or
                    // context race must never replace an authoritative upstream snapshot result.
                  }
                }
                const downloadArtifacts = riskLease?.artifacts() ?? []
                if (!artifactPlan) {
                  return withArtifactReferences(parsed, downloadArtifacts)
                }
                if (parsed.isError) {
                  await artifactPlan.reservation.discard().catch(() => undefined)
                  if (name === 'browser_pdf_save' && isPdfUnavailableResult(parsed)) {
                    return pdfUnavailableToolResult()
                  }
                  return {
                    content: [
                      {
                        type: 'text',
                        text: 'The managed browser Artifact operation failed before publication.'
                      }
                    ],
                    structuredContent: {
                      status: 'artifact_failed',
                      ...(downloadArtifacts.length > 0 ? { artifacts: downloadArtifacts } : {})
                    },
                    isError: true
                  }
                }
                if (operationSignal.aborted) throw cancellationError(operationSignal.reason)
                if (artifactPlan.hostWritesDiagnosticText) {
                  const text = parsed.content
                    .flatMap((block) => (block.type === 'text' ? [block.text] : []))
                    .join('\n')
                  await writeFile(
                    artifactPlan.reservation.managedPath,
                    text || 'No matching managed browser diagnostics.',
                    { flag: 'wx', mode: 0o600 }
                  )
                }
                if (operationSignal.aborted) throw cancellationError(operationSignal.reason)
                const artifact = await artifactPlan.reservation.commit()
                artifactCommitted = true
                if (operationSignal.aborted) throw cancellationError(operationSignal.reason)
                const screenshot = artifact.kind === 'image' ? artifact : undefined
                const readPathUnavailable =
                  screenshot && screenshot.sizeBytes > MAX_READ_IMAGE_BYTES
                    ? 'too_large'
                    : undefined
                const hostImagePublishPath =
                  screenshot && !readPathUnavailable
                    ? this.artifactBroker?.hostOwnedAbsolutePath(screenshot)
                    : undefined
                return artifactToolResult([artifact, ...downloadArtifacts], {
                  ...(hostImagePublishPath ? { hostImagePublishPath } : {}),
                  ...(readPathUnavailable ? { readPathUnavailable } : {})
                })
              } finally {
                if (artifactPlan && !artifactCommitted) {
                  await artifactPlan.reservation.discard().catch(() => undefined)
                }
                riskLease?.finish()
              }
            })().then(
              (result) => ({ ok: true, result }) as const,
              (error: unknown) => ({ ok: false, error }) as const
            )
            let targetFenceError: unknown
            try {
              sensitiveGrant?.targetBinding.finish()
            } catch (error) {
              targetFenceError = error
            }
            if (sensitiveGrant) {
              this.consumedSensitiveGrantIds.delete(sensitiveGrant.lease.grant.grantId)
            }
            await fileLease?.finish().catch(() => undefined)
            try {
              finishTargetCreationIntent?.()
            } finally {
              surfaceLease?.finish()
            }
            if (!outcome.ok) throw outcome.error
            if (targetFenceError) throw targetFenceError
            await acknowledgeDispatchPhase('response_received')
            responseReceived = true
            return outcome.result
          }),
        options.signal,
        options.timeoutMs,
        true
      )
    } catch (error) {
      const authorization = options.authorizationContext
      if (authorization) {
        this.sensitiveTargetBindings?.releaseToolCall({
          runId: authorization.runId,
          callId: authorization.callId
        })
        const release =
          this.releaseBrowserToolCall?.({
            runId: authorization.runId,
            toolCallId: authorization.callId
          }) ??
          this.artifactBroker?.releaseToolCall({
            runId: authorization.runId,
            toolCallId: authorization.callId
          })
        if (release)
          await settleWithin(
            release.catch(() => undefined),
            CONNECTION_CLOSE_SETTLE_MS
          )
      }
      const mapped = mapSafeHostError(error)
      if (mapped.dispatchCertainty) throw mapped
      if (
        dispatchStarted &&
        !responseReceived &&
        (mapped.code === 'mcp.builtin_playwright.timeout' ||
          mapped.code === 'mcp.builtin_playwright.cancelled')
      ) {
        throw new ManagedPlaywrightMcpHostError(
          'browser.risk_outcome_unknown',
          'possibly_dispatched'
        )
      }
      throw new ManagedPlaywrightMcpHostError(
        mapped.code,
        responseReceived
          ? 'response_received'
          : dispatchStarted
            ? 'possibly_dispatched'
            : 'definitely_not_dispatched'
      )
    }
  }

  private async serializeDispatch<T>(signal: AbortSignal, operation: () => Promise<T>): Promise<T> {
    let release!: () => void
    const slot = new Promise<void>((resolveSlot) => {
      release = resolveSlot
    })
    const previous = this.dispatchTail.catch(() => undefined)
    this.dispatchTail = previous.then(() => slot)
    try {
      await raceWithAbort(previous, signal)
      try {
        return await raceWithAbort(operation(), signal)
      } catch (error) {
        if (signal.aborted) {
          // An official handler can outlive request cancellation (for example, page JavaScript
          // returning a never-settling Promise). Revoke that whole automation generation before
          // releasing this slot so the orphan cannot share CDP authority with the next call.
          await this.disposeConnection(true, false)
        }
        throw error
      }
    } finally {
      release()
    }
  }

  private async acquireToolSurfaceLease(
    name: string,
    modelArguments: Readonly<Record<string, unknown>>
  ): Promise<ManagedPlaywrightToolSurfaceLease | undefined> {
    if (!this.surfaceGroup) return undefined
    if (
      name === 'browser_tabs' &&
      (modelArguments.action === 'select' || modelArguments.action === 'close') &&
      modelArguments.index !== undefined
    ) {
      if (!Number.isSafeInteger(modelArguments.index) || (modelArguments.index as number) < 0) {
        throw new ManagedPlaywrightMcpHostError('browser.target_closed')
      }
      if (!this.surfaceGroup.beginToolSurfaceLeaseByIndex) {
        // Compatibility for isolated Host unit adapters predating SurfaceGroup leasing. The
        // production BrowserSurfaceGroup always provides the exact-index CAS API.
        if (!this.surfaceGroup.beginToolSurfaceLease) return undefined
        throw new ManagedPlaywrightMcpHostError('browser.target_closed')
      }
      return await this.surfaceGroup.beginToolSurfaceLeaseByIndex(modelArguments.index as number)
    }
    const mode = toolSurfaceLeaseMode(name, modelArguments)
    if (mode === 'none') return undefined
    if (mode === 'creating') {
      return await this.surfaceGroup.beginToolSurfaceLease?.()
    }
    if (!this.surfaceGroup.beginExistingToolSurfaceLease) return undefined
    const lease = await this.surfaceGroup.beginExistingToolSurfaceLease()
    if (!lease && mode === 'optional_existing') {
      const retainedSurfaces = this.surfaceGroup.listSurfaces()
      if (retainedSurfaces.length === 0) return undefined
      // A retained user-visible page can temporarily have no trusted active selection while the
      // Renderer switches tasks or remounts the Browser panel. That is not the zero-page case:
      // fixed Playwright will reuse the retained Page instead of issuing Target.createTarget, so
      // a targetless creation lease would never acquire network authority for the actual guest.
      // Rebind only when one live page is uniquely identifiable. With multiple pages and no
      // trusted selection, fail closed instead of guessing a target or calling ensureSurface(),
      // which could reveal or create a page while trying to recover selection.
      if (retainedSurfaces.length !== 1 || !this.surfaceGroup.beginToolSurfaceLeaseByIndex) {
        throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
      }
      return await this.surfaceGroup.beginToolSurfaceLeaseByIndex(retainedSurfaces[0]!.index)
    }
    if (!lease && mode === 'existing') {
      throw new ManagedPlaywrightMcpHostError('browser.target_closed')
    }
    return lease ?? undefined
  }

  private async executeManagedTargetCreatingTool(input: {
    toolName?: 'browser_tabs' | 'browser_navigate'
    arguments: Record<string, unknown>
    options: {
      authorizationContext?: BrowserRiskAuthorizationContext
      parentRequestId?: string
    }
    signal: AbortSignal
    markDispatched(): Promise<void>
    markResponseReceived(): void
  }): Promise<ManagedPlaywrightCallResult> {
    const authorization = input.options.authorizationContext
    const parentRequestId = input.options.parentRequestId
    const beginTargetCreationOperation = this.beginTargetCreationOperation
    const surfaceGroup = this.surfaceGroup
    if (
      !authorization ||
      !parentRequestId ||
      !beginTargetCreationOperation ||
      !surfaceGroup?.beginTargetCreationIntent
    ) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    const toolName = input.toolName ?? 'browser_tabs'
    // The fixed tabs schema shares `url` across its action union. Official list ignores it; only
    // `new` may navigate. browser_navigate always carries the authoritative destination URL.
    const requestedUrl =
      toolName === 'browser_navigate'
        ? input.arguments.url
        : input.arguments.action === 'new'
          ? input.arguments.url
          : undefined
    if (requestedUrl !== undefined && typeof requestedUrl !== 'string') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    const url = requestedUrl ?? 'about:blank'
    const riskLease = await beginTargetCreationOperation({
      authorizationContext: authorization,
      parentRequestId,
      signal: input.signal
    })
    let authority: BrowserTargetCreationAuthority | undefined
    let finishIntent: (() => void) | undefined
    try {
      await riskLease.ready()
      authority = await riskLease.beginTargetCreationAuthority({
        action: 'new',
        runId: authorization.runId,
        activationId: authorization.activationId,
        capabilityId: authorization.capabilityId,
        toolCallId: authorization.callId,
        toolId: toolName,
        url
      })
      finishIntent = surfaceGroup.beginTargetCreationIntent('interactive', authority)

      const connection = await this.ensureConnected()
      await this.ensureOfficialCatalog(connection, input.signal)
      // Intent registration and an empty-context/catalog handshake are reversible Host setup.
      // Cross the side-effect boundary only immediately before fixed official browser_tabs can
      // issue Target.createTarget/navigation, so an earlier connection failure stays definite.
      await input.markDispatched()
      riskLease.markDispatched()
      const officialResult = await this.callOfficialTool(
        connection,
        {
          name: toolName,
          arguments: input.arguments
        },
        input.signal
      )
      const result = adaptReviewedToolResult(toolName, officialResult)
      input.markResponseReceived()
      await riskLease.settle()
      const failure = riskLease.failure()
      if (failure) return riskFailureResult(failure)
      if (input.signal.aborted) throw cancellationError(input.signal.reason)
      return withArtifactReferences(result, riskLease.artifacts())
    } catch (error) {
      try {
        await riskLease.settle()
      } catch {
        const pendingFailure = riskLease.failure()
        if (pendingFailure) return riskFailureResult(pendingFailure)
      }
      const failure = riskLease.failure()
      if (failure) return riskFailureResult(failure)
      throw error
    } finally {
      if (finishIntent) finishIntent()
      else authority?.finish()
      riskLease.finish()
    }
  }

  private async getExactManagedPage(
    surfaceLease: ManagedPlaywrightToolSurfaceLease | undefined
  ): Promise<Page> {
    const context = await this.getManagedBrowserContext()
    if (surfaceLease) {
      const page = context.pages()[await surfaceLease.resolveIndex()]
      if (!page || (typeof page.isClosed === 'function' && page.isClosed())) {
        throw new ManagedPlaywrightMcpHostError('browser.target_closed')
      }
      return page
    }
    // Unit-only/legacy adapters may omit SurfaceGroup. Fail closed on ambiguity instead of
    // silently applying a page operation to context.pages()[0] in a multi-target context.
    const pages = context
      .pages()
      .filter((page) => typeof page.isClosed !== 'function' || !page.isClosed())
    if ((!this.surfaceGroup || !this.surfaceGroup.beginToolSurfaceLease) && pages.length === 1) {
      return pages[0]!
    }
    throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
  }

  private async callOfficialTool(
    connection: ActiveConnection,
    request: Parameters<ManagedMcpClient['callTool']>[0],
    signal: AbortSignal,
    timeoutMs = this.toolTimeoutMs,
    onResponseReceived?: () => void | Promise<void>
  ): Promise<ManagedPlaywrightCallResult> {
    let rawResponseReceived = false
    let rawResult: unknown
    try {
      rawResult = await connection.client.callTool(request, undefined, {
        signal,
        timeout: boundedTimeout(timeoutMs),
        resetTimeoutOnProgress: false
      })
      rawResponseReceived = true
      await onResponseReceived?.()
    } catch (error) {
      if (!rawResponseReceived && !signal.aborted) {
        console.warn('[managed-playwright] official call rejected before response', {
          reason: safeOfficialCallFailureReason(error),
          tool: request.name
        })
        // The SDK exposes authoritative Tool-level failures as resolved `isError` results. A
        // rejected request therefore has no authoritative response and may leave the official
        // server's lazy shared-context promise or its in-memory transport unusable. Retire this
        // generation exactly once, but never replay the current invocation.
        await this.retireStaleConnection(connection).catch(() => undefined)
      }
      throw error
    }
    const result = parseBoundedToolResult(rawResult)
    if (isTerminalManagedConnectionResult(result)) {
      // A resolved Tool error is authoritative for this invocation and must never be replayed.
      // Terminal connection failures still retire the generation before another independent
      // call can reuse it. Ordinary Tool-level isError results deliberately remain reusable.
      await this.retireStaleConnection(connection).catch(() => undefined)
    }
    return result
  }

  private async selectOfficialSurface(
    connection: ActiveConnection,
    index: number,
    signal: AbortSignal,
    markAuthoritativeFailure: () => Promise<void>
  ): Promise<void> {
    if (!Number.isSafeInteger(index) || index < 0) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    let rawResponseReceived = false
    let selected: ManagedPlaywrightCallResult
    try {
      selected = await this.callOfficialTool(
        connection,
        { name: 'browser_tabs', arguments: { action: 'select', index } },
        signal,
        this.toolTimeoutMs,
        () => {
          rawResponseReceived = true
        }
      )
    } catch (error) {
      if (!rawResponseReceived) throw error
      await markAuthoritativeFailure()
      const mapped = mapSafeHostError(error)
      throw new ManagedPlaywrightMcpHostError(mapped.code, 'response_received')
    }
    if (selected.isError) {
      await markAuthoritativeFailure()
      throw new ManagedPlaywrightMcpHostError('browser.target_closed', 'response_received')
    }
  }

  private async hydrateOfficialRetainedContext(
    connection: ActiveConnection,
    signal: AbortSignal,
    markDispatched: () => Promise<void>,
    markAuthoritativeFailure: () => Promise<void>
  ): Promise<void> {
    if (connection.context) return
    await markDispatched()
    let rawResponseReceived = false
    let listed: ManagedPlaywrightCallResult
    try {
      listed = await this.callOfficialTool(
        connection,
        { name: 'browser_tabs', arguments: { action: 'list' } },
        signal,
        this.toolTimeoutMs,
        () => {
          rawResponseReceived = true
        }
      )
    } catch (error) {
      // A rejected SDK request has no authoritative Tool response and remains replay-unsafe /
      // possibly dispatched. Parsing failure after a resolved raw response is authoritative.
      if (!rawResponseReceived) throw error
      await markAuthoritativeFailure()
      const mapped = mapSafeHostError(error)
      throw new ManagedPlaywrightMcpHostError(mapped.code, 'response_received')
    }
    if (listed.isError) {
      await markAuthoritativeFailure()
      throw new ManagedPlaywrightMcpHostError('browser.target_closed', 'response_received')
    }
  }

  private assertRunStateAccess(runId: string | undefined): void {
    if (this.stateOwnerRunId && runId !== this.stateOwnerRunId) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
  }

  private claimStateOwner(runId: string): void {
    if (this.stateOwnerRunId && this.stateOwnerRunId !== runId) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
    this.stateOwnerRunId = runId
  }

  private maybeReleaseStateOwner(runId: string): void {
    if (
      this.stateOwnerRunId === runId &&
      !this.offlineRuns.has(runId) &&
      !this.routeDefinitions.some((route) => route.runId === runId) &&
      this.tracing?.owner.runId !== runId
    ) {
      this.stateOwnerRunId = undefined
    }
  }

  private async prepareSensitiveGrant(
    reviewed: ManagedPlaywrightToolManifestEntry,
    modelArguments: Record<string, unknown>,
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  ): Promise<PreparedSensitiveGrant | undefined> {
    if (!sensitiveToolNeedsGrant(reviewed.rawName, modelArguments)) {
      validateSensitiveToolGrant({
        reviewed,
        modelArguments,
        authorizationContext,
        activeOrigin: null,
        consumedGrantIds: this.consumedSensitiveGrantIds
      })
      return undefined
    }
    const policy = sensitivePolicyForTool(reviewed.rawName)
    if (!policy) throw new ManagedPlaywrightSensitiveGrantError('drifted')
    const profileScoped =
      sensitiveBindingScopeForInvocation(reviewed.rawName, modelArguments) ===
      'managed_browser_profile'
    const target = profileScoped ? undefined : this.getActiveBrowserTarget()
    const lease = validateSensitiveToolGrant({
      reviewed,
      modelArguments,
      authorizationContext,
      activeOrigin: target?.origin ?? null,
      consumedGrantIds: this.consumedSensitiveGrantIds
    })
    if (!lease) throw new ManagedPlaywrightSensitiveGrantError('missing')
    if (!authorizationContext || !this.sensitiveTargetBindings) {
      throw new ManagedPlaywrightSensitiveGrantError('missing')
    }
    let targetBinding: ManagedPlaywrightSensitiveTargetBindingLease
    try {
      targetBinding = this.sensitiveTargetBindings.acquire(authorizationContext)
    } catch (error) {
      if (error instanceof ManagedPlaywrightSensitiveTargetBindingError) {
        throw new ManagedPlaywrightSensitiveGrantError(
          error.code === 'expired'
            ? 'expired'
            : error.code === 'origin_drifted'
              ? 'origin_drifted'
              : error.code === 'reused'
                ? 'reused'
                : 'drifted'
        )
      }
      throw error
    }
    if (profileScoped) {
      if (targetBinding.target || lease.grant.origin !== null) {
        throw new ManagedPlaywrightSensitiveGrantError('drifted')
      }
    } else {
      if (
        !target ||
        !targetBinding.target ||
        target.surfaceId !== targetBinding.target.surfaceId ||
        target.generation !== targetBinding.target.generation ||
        target.navigationEpoch !== targetBinding.target.navigationEpoch ||
        target.origin !== targetBinding.target.origin
      ) {
        throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
      }
    }
    return { lease, target, targetBinding }
  }

  private async prepareSensitiveFiles(input: {
    name: string
    arguments: Record<string, unknown>
    authorizationContext: BrowserRiskAuthorizationContext | undefined
    connection: ActiveConnection
    preparedHandles?: readonly string[]
  }): Promise<{ arguments: Record<string, unknown>; lease?: PreparedSensitiveFileLease }> {
    let handles: readonly string[] | undefined
    if (input.name === 'browser_file_upload' || input.name === 'browser_drop') {
      if (input.preparedHandles) {
        handles = input.preparedHandles
      } else {
        if (input.arguments.paths === undefined) return { arguments: input.arguments }
        if (!Array.isArray(input.arguments.paths)) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        handles = input.arguments.paths.map((value) => {
          if (typeof value !== 'string') {
            throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
          }
          return value
        })
      }
      if (handles.length === 0) return { arguments: input.arguments }
    } else if (input.name === 'browser_set_storage_state') {
      if (input.preparedHandles) {
        handles = input.preparedHandles
      } else if (typeof input.arguments.filename !== 'string') {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      } else {
        handles = [input.arguments.filename]
      }
      if (handles.length !== 1) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
    } else {
      if (input.preparedHandles) {
        throw new ManagedPlaywrightSensitiveGrantError('drifted')
      }
      return { arguments: input.arguments }
    }
    if (!this.fileBroker) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    const owner = this.fileOwner(input.authorizationContext)
    const lease = await this.fileBroker.consumeForRead({
      owner,
      handles
    })
    let workspaceDirectory: string | undefined
    try {
      workspaceDirectory = await mkdtemp(join(input.connection.outputDirectory, '.file-input-'))
      await chmod(workspaceDirectory, 0o700)
      const stagedPaths: string[] = []
      for (const [index, filePath] of lease.paths.entries()) {
        const displayName = lease.references[index]?.displayName
        if (
          !displayName ||
          basename(displayName) !== displayName ||
          displayName === '.' ||
          displayName === '..'
        ) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        const itemDirectory = join(workspaceDirectory, String(index))
        await mkdir(itemDirectory, { mode: 0o700 })
        const stagedPath = join(itemDirectory, displayName)
        await copyFile(filePath, stagedPath)
        await chmod(stagedPath, 0o600)
        stagedPaths.push(stagedPath)
      }
      if (input.name === 'browser_file_upload' || input.name === 'browser_drop') {
        const target = this.getActiveBrowserTarget()
        await lease.finish()
        const releaseWorkspace = input.connection.outputLifetime.retainDirectory(workspaceDirectory)
        let retained: BrowserFileRetainedLease
        try {
          retained = await this.fileBroker.retainConsumedFiles({
            owner,
            surfaceId: target.surfaceId,
            generation: target.generation,
            references: lease.references,
            dispose: releaseWorkspace
          })
        } catch (error) {
          await releaseWorkspace()
          throw error
        }
        return {
          arguments: {
            ...input.arguments,
            paths: stagedPaths,
            _meta: { cwd: workspaceDirectory }
          },
          lease: {
            ...lease,
            paths: stagedPaths,
            markDispatched: retained.markDispatched,
            finish: retained.finish
          }
        }
      }
      let finished = false
      const stagedLease: BrowserFileReadLease = {
        ...lease,
        paths: stagedPaths,
        finish: async () => {
          if (finished) return
          finished = true
          await Promise.allSettled([
            lease.finish(),
            rm(workspaceDirectory!, { force: true, recursive: true })
          ])
        }
      }
      return {
        arguments:
          input.name === 'browser_set_storage_state'
            ? {
                ...input.arguments,
                filename: stagedPaths[0],
                _meta: { cwd: workspaceDirectory }
              }
            : input.arguments,
        lease: stagedLease
      }
    } catch (error) {
      await Promise.allSettled([
        lease.finish(),
        ...(workspaceDirectory ? [rm(workspaceDirectory, { force: true, recursive: true })] : [])
      ])
      throw error
    }
  }

  private fileOwner(
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  ): BrowserFileOwner {
    if (!authorizationContext) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    return {
      runId: authorizationContext.runId,
      activationId: authorizationContext.activationId,
      capabilityId: 'browser_automation',
      toolCallId: authorizationContext.callId
    }
  }

  private async appendSafeFrameEditorCandidates(
    result: ManagedPlaywrightCallResult,
    page: Page,
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  ): Promise<ManagedPlaywrightCallResult> {
    await this.clearFrameEditorCandidates()
    const target = this.getActiveBrowserTarget()
    const candidates = await probeBrowserFrameEditors(page)
    const registeredTargets: string[] = []
    try {
      const projection = formatBrowserFrameEditorCandidates(candidates, (candidate) => {
        const opaqueTarget = `${FRAME_EDITOR_TARGET_PREFIX}${randomUUID()}`
        this.frameEditorCandidates.set(opaqueTarget, {
          activationId: authorizationContext?.activationId,
          element: candidate.element,
          expiresAtMs: Date.now() + FRAME_EDITOR_TARGET_TTL_MS,
          frame: candidate.frame,
          generation: target.generation,
          kind: candidate.kind,
          runId: authorizationContext?.runId,
          surfaceId: target.surfaceId
        })
        registeredTargets.push(opaqueTarget)
        return opaqueTarget
      })
      if (!projection) return result
      return {
        ...result,
        content: [...result.content, { type: 'text', text: projection }]
      }
    } catch (error) {
      await Promise.allSettled(
        registeredTargets.map(async (opaqueTarget) => {
          const candidate = this.frameEditorCandidates.get(opaqueTarget)
          this.frameEditorCandidates.delete(opaqueTarget)
          await candidate?.element.dispose()
        })
      )
      throw error
    }
  }

  private async clearFrameEditorCandidates(): Promise<void> {
    const candidates = [...this.frameEditorCandidates.values()]
    this.frameEditorCandidates.clear()
    await Promise.allSettled(candidates.map(async (candidate) => candidate.element.dispose()))
  }

  private async clearFrameEditorCandidatesForRun(runId: string): Promise<void> {
    const candidates = [...this.frameEditorCandidates.entries()].filter(
      ([, candidate]) => candidate.runId === runId
    )
    for (const [opaqueTarget] of candidates) this.frameEditorCandidates.delete(opaqueTarget)
    await Promise.allSettled(candidates.map(([, candidate]) => candidate.element.dispose()))
  }

  private async takeFrameEditorCandidate(
    opaqueTarget: string,
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  ): Promise<RegisteredFrameEditorCandidate> {
    const candidate = this.frameEditorCandidates.get(opaqueTarget)
    this.frameEditorCandidates.delete(opaqueTarget)
    if (!candidate) throw new ManagedFrameEditorTargetError('stale_frame_ref')
    const reject = async (
      code: ManagedFrameFailureCode
    ): Promise<RegisteredFrameEditorCandidate> => {
      await candidate.element.dispose().catch(() => undefined)
      throw new ManagedFrameEditorTargetError(code)
    }
    if (
      candidate.expiresAtMs <= Date.now() ||
      candidate.runId !== authorizationContext?.runId ||
      candidate.activationId !== authorizationContext?.activationId
    ) {
      return reject('stale_frame_ref')
    }
    const active = this.getActiveBrowserTarget()
    if (active.surfaceId !== candidate.surfaceId || active.generation !== candidate.generation) {
      return reject('stale_frame_ref')
    }
    if (candidate.frame.isDetached()) return reject('frame_detached')
    let editableKind: BrowserFrameEditorKind | null = null
    try {
      editableKind = await candidate.element.evaluate((element) => {
        if (!element.isConnected) return null
        if (element instanceof HTMLInputElement) {
          return ['email', 'number', 'password', 'search', 'tel', 'text', 'url'].includes(
            element.type
          ) &&
            !element.disabled &&
            !element.readOnly
            ? 'input'
            : null
        }
        if (element instanceof HTMLTextAreaElement) {
          return !element.disabled && !element.readOnly ? 'textarea' : null
        }
        return element instanceof HTMLElement && element.isContentEditable
          ? 'contenteditable'
          : null
      })
    } catch {
      return reject(candidate.frame.isDetached() ? 'frame_detached' : 'stale_frame_ref')
    }
    if (editableKind !== candidate.kind) return reject('frame_not_editable')
    return candidate
  }

  private getActiveBrowserTarget(): {
    surfaceId: string
    generation: number
    navigationEpoch: number
    origin: string
  } {
    if (!this.surfaceGroup) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    const target = this.surfaceGroup.getSensitiveTargetIdentity()
    if (!target) throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
    return target
  }

  private assertSensitiveDispatchTarget(expected: {
    surfaceId: string
    generation: number
    navigationEpoch: number
    origin: string
  }): void {
    const current = this.getActiveBrowserTarget()
    if (
      current.surfaceId !== expected.surfaceId ||
      current.generation !== expected.generation ||
      current.navigationEpoch !== expected.navigationEpoch ||
      current.origin !== expected.origin
    ) {
      throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
    }
  }

  private async executeHostAdapter(
    input: HostAdapterInput
  ): Promise<ManagedPlaywrightCallResult | undefined> {
    const frameEditorResult = await this.executeFrameEditorAdapter(input)
    if (frameEditorResult) return frameEditorResult
    switch (input.name) {
      case 'browser_resize': {
        const width = expectPositiveDimension(input.arguments.width)
        const height = expectPositiveDimension(input.arguments.height)
        const resize = input.surfaceLease?.resizeSurface
          ? (dimensions: { width: number; height: number }) =>
              input.surfaceLease!.resizeSurface!(dimensions)
          : !this.surfaceGroup?.beginToolSurfaceLease && this.surfaceGroup?.resizeActiveSurface
            ? (dimensions: { width: number; height: number }) =>
                this.surfaceGroup!.resizeActiveSurface!(dimensions)
            : undefined
        if (!resize) {
          throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
        }
        await input.markDispatched()
        const actual = await resize({ width, height })
        return {
          content: [
            {
              type: 'text',
              text: `Managed browser viewport resized to ${actual.width}x${actual.height}.`
            }
          ],
          structuredContent: { width: actual.width, height: actual.height },
          isError: false
        }
      }
      case 'browser_drop': {
        const adapted = await this.executeBrokeredPathDropAdapter(input)
        if (adapted) return adapted
        return undefined
      }
      case 'browser_network_state_set': {
        const runId = requiredRunId(input.options.authorizationContext)
        const state = input.arguments.state
        if (state !== 'online' && state !== 'offline') {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        await this.getManagedBrowserContext()
        const wasOffline = this.offlineRuns.has(runId)
        if (state === 'offline') this.claimStateOwner(runId)
        await input.markDispatched()
        if (state === 'offline') this.offlineRuns.add(runId)
        else this.offlineRuns.delete(runId)
        try {
          await this.reconcileManagedNetworkState()
        } catch (error) {
          if (wasOffline) this.offlineRuns.add(runId)
          else this.offlineRuns.delete(runId)
          await this.reconcileManagedNetworkState().catch(() => undefined)
          this.maybeReleaseStateOwner(runId)
          throw error
        }
        this.maybeReleaseStateOwner(runId)
        return textToolResult(`Managed browser network is now ${state}.`)
      }
      case 'browser_route': {
        const runId = requiredRunId(input.options.authorizationContext)
        const definition = managedRouteDefinition(runId, input.arguments)
        await this.getManagedBrowserContext()
        this.claimStateOwner(runId)
        await input.markDispatched()
        this.routeDefinitions.push(definition)
        try {
          await this.reconcileManagedNetworkState()
        } catch (error) {
          const index = this.routeDefinitions.indexOf(definition)
          if (index >= 0) this.routeDefinitions.splice(index, 1)
          await this.reconcileManagedNetworkState().catch(() => undefined)
          this.maybeReleaseStateOwner(runId)
          throw error
        }
        return textToolResult(`Route added for pattern: ${definition.pattern}`)
      }
      case 'browser_route_list': {
        const runId = requiredRunId(input.options.authorizationContext)
        await this.getManagedBrowserContext()
        const routes = this.routeDefinitions.filter((route) => route.runId === runId)
        return textToolResult(
          routes.length === 0
            ? 'No active routes'
            : routes.map((route, index) => managedRouteListLine(route, index)).join('\n')
        )
      }
      case 'browser_unroute': {
        const runId = requiredRunId(input.options.authorizationContext)
        const pattern = input.arguments.pattern
        if (pattern !== undefined && typeof pattern !== 'string') {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        await this.getManagedBrowserContext()
        await input.markDispatched()
        let removed = 0
        for (let index = this.routeDefinitions.length - 1; index >= 0; index -= 1) {
          const route = this.routeDefinitions[index]
          if (route.runId === runId && (pattern === undefined || route.pattern === pattern)) {
            this.routeDefinitions.splice(index, 1)
            removed += 1
          }
        }
        await this.reconcileManagedNetworkState()
        this.maybeReleaseStateOwner(runId)
        return textToolResult(
          pattern === undefined
            ? `Removed all ${removed} route(s)`
            : `Removed ${removed} route(s) for pattern: ${pattern}`
        )
      }
      case 'browser_start_tracing':
        return this.startTracing(input)
      case 'browser_stop_tracing':
        return this.stopTracing(input)
      default:
        return undefined
    }
  }

  private async executeBrokeredPathDropAdapter(
    input: HostAdapterInput
  ): Promise<ManagedPlaywrightCallResult | undefined> {
    const paths = input.arguments.paths
    if (!Array.isArray(paths) || paths.length === 0) return undefined
    if (paths.some((value) => typeof value !== 'string')) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }

    let totalBytes = 0
    for (const path of paths as string[]) {
      const file = await stat(path)
      totalBytes += file.size
      if (!file.isFile() || !Number.isSafeInteger(totalBytes)) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
    }

    const target = input.arguments.target
    const data = input.arguments.data
    const dataRecord = data === undefined ? undefined : expectRecordWithoutThrow(data)
    if (
      typeof target !== 'string' ||
      (data !== undefined &&
        (dataRecord === undefined ||
          Object.values(dataRecord).some((value) => typeof value !== 'string')))
    ) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }

    const page = await this.getExactManagedPage(input.surfaceLease)
    const locator = managedDropTargetLocator(page, target)
    const targetElement = await locator.elementHandle()
    if (!targetElement) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }

    let inputElement: ElementHandle<HTMLInputElement> | undefined
    try {
      await input.markDispatched()
      inputElement = (
        await targetElement.evaluateHandle((element) => {
          const fileInput = element.ownerDocument.createElement('input')
          fileInput.type = 'file'
          fileInput.multiple = true
          fileInput.hidden = true
          fileInput.setAttribute('aria-hidden', 'true')
          element.ownerDocument.documentElement.append(fileInput)
          return fileInput
        })
      ).asElement() as ElementHandle<HTMLInputElement> | undefined
      if (!inputElement) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
      }
      await inputElement.setInputFiles(paths as string[])
      if (input.signal.aborted) throw cancellationError(input.signal.reason)
      const outcome = await targetElement.evaluate(
        (
          element,
          options: {
            data: Record<string, string> | undefined
            fileInput: HTMLInputElement
          }
        ) => {
          const transfer = new DataTransfer()
          for (const file of Array.from(options.fileInput.files ?? [])) {
            transfer.items.add(file)
          }
          for (const [mimeType, value] of Object.entries(options.data ?? {})) {
            transfer.setData(mimeType, value)
          }
          const rect = element.getBoundingClientRect()
          const eventInit: DragEventInit = {
            bubbles: true,
            cancelable: true,
            composed: true,
            clientX: rect.left + rect.width / 2,
            clientY: rect.top + rect.height / 2,
            dataTransfer: transfer
          }
          element.dispatchEvent(new DragEvent('dragenter', eventInit))
          const dragover = new DragEvent('dragover', eventInit)
          element.dispatchEvent(dragover)
          if (!dragover.defaultPrevented) {
            return { accepted: false, fileCount: transfer.files.length }
          }
          element.dispatchEvent(new DragEvent('drop', eventInit))
          return { accepted: true, fileCount: transfer.files.length }
        },
        {
          data: dataRecord as Record<string, string> | undefined,
          fileInput: inputElement
        }
      )
      if (input.signal.aborted) throw cancellationError(input.signal.reason)
      if (!outcome.accepted) {
        return {
          content: [
            {
              type: 'text',
              text: 'The managed browser drop target did not accept the dragover event.'
            }
          ],
          structuredContent: {
            status: 'drop_not_accepted',
            dispatchCertainty: 'response_received'
          },
          isError: true
        }
      }
      await inputElement.evaluate((element) => element.remove()).catch(() => undefined)
      await inputElement.dispose().catch(() => undefined)
      inputElement = undefined
      const snapshot = adaptReviewedToolResult(
        'browser_snapshot',
        await this.callOfficialTool(
          input.connection,
          { name: 'browser_snapshot', arguments: {} },
          input.signal
        )
      )
      if (input.signal.aborted) throw cancellationError(input.signal.reason)
      const completion = {
        type: 'text' as const,
        text: `Dropped ${outcome.fileCount} approved file(s) on the managed browser target.`
      }
      return {
        content: snapshot.isError ? [completion] : [completion, ...snapshot.content],
        structuredContent: {
          status: 'completed',
          fileCount: outcome.fileCount,
          snapshotIncluded: !snapshot.isError
        },
        isError: false
      }
    } finally {
      await inputElement?.evaluate((element) => element.remove()).catch(() => undefined)
      await inputElement?.dispose().catch(() => undefined)
      await targetElement.dispose().catch(() => undefined)
    }
  }

  private async executeFrameEditorAdapter(
    input: HostAdapterInput
  ): Promise<ManagedPlaywrightCallResult | undefined> {
    const opaqueTargets: string[] = []
    if (input.name === 'browser_type' || input.name === 'browser_click') {
      const target = input.arguments.target
      if (typeof target !== 'string' || !target.startsWith(FRAME_EDITOR_TARGET_PREFIX)) {
        return undefined
      }
      opaqueTargets.push(target)
    } else if (input.name === 'browser_fill_form') {
      if (!Array.isArray(input.arguments.fields)) return undefined
      const targets = input.arguments.fields.map((field) =>
        field &&
        typeof field === 'object' &&
        typeof (field as Record<string, unknown>).target === 'string'
          ? ((field as Record<string, unknown>).target as string)
          : ''
      )
      const managedTargets = targets.filter((target) =>
        target.startsWith(FRAME_EDITOR_TARGET_PREFIX)
      )
      if (managedTargets.length === 0) return undefined
      if (managedTargets.length !== targets.length) {
        return managedFrameFailureResult('stale_frame_ref')
      }
      opaqueTargets.push(...managedTargets)
    } else {
      return undefined
    }

    const candidates: RegisteredFrameEditorCandidate[] = []
    try {
      for (const opaqueTarget of opaqueTargets) {
        candidates.push(
          await this.takeFrameEditorCandidate(opaqueTarget, input.options.authorizationContext)
        )
      }
    } catch (error) {
      await Promise.allSettled(candidates.map(async (candidate) => candidate.element.dispose()))
      if (error instanceof ManagedFrameEditorTargetError) {
        return managedFrameFailureResult(error.code)
      }
      return managedFrameFailureResult('frame_input_delivery_failed')
    }

    const frameFields =
      input.name === 'browser_fill_form'
        ? (input.arguments.fields as Array<Record<string, unknown>>)
        : undefined
    if (frameFields?.some((field) => field.type !== 'textbox' || typeof field.value !== 'string')) {
      await Promise.allSettled(candidates.map(async (candidate) => candidate.element.dispose()))
      return managedFrameFailureResult('frame_not_editable')
    }

    await input.markDispatched()
    try {
      if (input.name === 'browser_type') {
        const text = input.arguments.text
        if (typeof text !== 'string') return managedFrameFailureResult('frame_not_editable')
        if (input.arguments.slowly === true) {
          await candidates[0].element.type(text)
        } else {
          await candidates[0].element.fill(text)
        }
        if (input.arguments.submit === true) await candidates[0].element.press('Enter')
        return textToolResult('The managed iframe editor was filled.')
      }
      if (input.name === 'browser_click') {
        await candidates[0].element.click({
          ...(typeof input.arguments.button === 'string'
            ? { button: input.arguments.button as 'left' | 'middle' | 'right' }
            : {}),
          ...(input.arguments.doubleClick === true ? { clickCount: 2 } : {}),
          ...(Array.isArray(input.arguments.modifiers)
            ? {
                modifiers: input.arguments.modifiers as Array<
                  'Alt' | 'Control' | 'ControlOrMeta' | 'Meta' | 'Shift'
                >
              }
            : {})
        })
        return textToolResult('The managed iframe editor was clicked.')
      }
      for (let index = 0; index < frameFields!.length; index += 1) {
        await candidates[index].element.fill(frameFields![index].value as string)
      }
      return textToolResult('The managed iframe editors were filled.')
    } catch {
      return managedFrameFailureResult(
        candidates.some((candidate) => candidate.frame.isDetached())
          ? 'frame_detached'
          : 'frame_input_delivery_failed'
      )
    } finally {
      await Promise.allSettled(candidates.map(async (candidate) => candidate.element.dispose()))
    }
  }

  private async startTracing(input: HostAdapterInput): Promise<ManagedPlaywrightCallResult> {
    if (this.tracing) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
    const runId = requiredRunId(input.options.authorizationContext)
    this.claimStateOwner(runId)
    let reservation: BrowserArtifactReservation | undefined
    try {
      const context = await this.getManagedBrowserContext()
      const owner = this.artifactOwner(input.options, undefined, 'managed_browser_profile')
      const session = input.connection.outputSession
      if (!session) throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
      reservation = await session.reserveFile({
        owner,
        kind: 'trace',
        mimeType: 'application/zip',
        suggestedFileName: 'browser-trace.zip'
      })
      await input.markDispatched()
      await context.tracing.start({ screenshots: true, snapshots: true, sources: false })
      this.tracing = { context, owner, reservation }
      return textToolResult('Managed browser trace recording started.')
    } catch (error) {
      await reservation?.discard().catch(() => undefined)
      this.maybeReleaseStateOwner(runId)
      throw error
    }
  }

  private async stopTracing(input: HostAdapterInput): Promise<ManagedPlaywrightCallResult> {
    const tracing = this.tracing
    const runId = requiredRunId(input.options.authorizationContext)
    if (!tracing || tracing.owner.runId !== runId) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    await input.markDispatched()
    this.tracing = undefined
    try {
      await tracing.context.tracing.stop({ path: tracing.reservation.managedPath })
      const artifact = await tracing.reservation.commit()
      this.maybeReleaseStateOwner(runId)
      return artifactToolResult(artifact)
    } catch (error) {
      await tracing.reservation.discard().catch(() => undefined)
      this.maybeReleaseStateOwner(runId)
      throw error
    }
  }

  private artifactOwner(
    input: HostAdapterInput['options'],
    surfaceLease: ManagedPlaywrightToolSurfaceLease | undefined,
    scope: 'managed_surface' | 'managed_browser_profile'
  ): BrowserArtifactOwner {
    const authorization = input.authorizationContext
    if (!authorization) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    const identity =
      scope === 'managed_browser_profile'
        ? { surfaceId: 'managed-browser-profile', generation: 1 }
        : surfaceLease
          ? { surfaceId: surfaceLease.surfaceId, generation: surfaceLease.generation }
          : !this.surfaceGroup?.beginToolSurfaceLease
            ? this.getActiveSurfaceIdentity?.()
            : undefined
    if (!identity) throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    return {
      runId: authorization.runId,
      activationId: authorization.activationId,
      capabilityId: 'browser_automation',
      surfaceId: identity.surfaceId,
      generation: identity.generation,
      toolCallId: authorization.callId
    }
  }

  private async prepareUpstreamArtifactPlan(input: {
    name: string
    arguments: Record<string, unknown>
    modelArguments: Record<string, unknown>
    options: HostAdapterInput['options']
    connection: ActiveConnection
    surfaceLease?: ManagedPlaywrightToolSurfaceLease
  }): Promise<ManagedUpstreamArtifactPlan | undefined> {
    const spec = artifactSpec(input.name, input.modelArguments)
    if (!spec) return undefined
    await this.getManagedBrowserContext()
    const session = input.connection.outputSession
    if (!session) throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    const reservation = await session.reserveFile({
      owner: this.artifactOwner(
        input.options,
        input.surfaceLease,
        input.name === 'browser_storage_state' ? 'managed_browser_profile' : 'managed_surface'
      ),
      kind: spec.kind,
      mimeType: spec.mimeType,
      suggestedFileName: spec.suggestedFileName,
      ...(spec.allowPreview === false ? { allowPreview: false } : {})
    })
    const hostWritesDiagnosticText = spec.kind === 'console' || spec.kind === 'network'
    const adaptedArguments = { ...input.arguments }
    if (hostWritesDiagnosticText) delete adaptedArguments.filename
    return {
      hostWritesDiagnosticText,
      reservation,
      serverArguments: hostWritesDiagnosticText
        ? adaptedArguments
        : {
            ...input.arguments,
            filename: reservation.fileName,
            _meta: { cwd: input.connection.outputDirectory }
          }
    }
  }

  /** Stops managed automation without closing the user's right-sidebar browser page. */
  async close(): Promise<void> {
    if (this.closed) return
    this.closed = true
    await this.cleanupManagedState()
    await this.disposeConnection(true)
    await Promise.allSettled(
      [...this.activationIds].map(
        (activationId) =>
          this.releaseBrowserCapability?.(activationId) ??
          this.artifactBroker?.releaseCapability(activationId)
      )
    )
    this.activationIds.clear()
  }

  private async getManagedBrowserContext(): Promise<BrowserContext> {
    const context = await this.getBrowserContext()
    await this.applyManagedNetworkState(context)
    return context
  }

  private async applyManagedNetworkState(context: BrowserContext): Promise<void> {
    let applied = this.contextNetworkState.get(context)
    if (!applied) {
      const handleClose = (): void => this.handleManagedContextClosed(context)
      applied = { handleClose, routes: new Map(), offline: false }
      this.contextNetworkState.set(context, applied)
      context.once('close', handleClose)
    }
    for (const definition of this.routeDefinitions) {
      if (applied.routes.has(definition)) continue
      const handler = async (route: Route): Promise<void> => {
        if (definition.body !== undefined || definition.status !== undefined) {
          await route.fulfill({
            status: definition.status ?? 200,
            contentType: definition.contentType,
            body: definition.body
          })
          return
        }
        const headers = { ...route.request().headers() }
        if (definition.addHeaders) {
          for (const [name, value] of Object.entries(definition.addHeaders)) {
            headers[name] = value
          }
        }
        if (definition.removeHeaders) {
          for (const name of definition.removeHeaders) delete headers[name.toLowerCase()]
        }
        await route.continue({ headers })
      }
      await context.route(definition.pattern, handler)
      applied.routes.set(definition, handler)
    }
    for (const [definition, handler] of [...applied.routes]) {
      if (this.routeDefinitions.includes(definition)) continue
      await context.unroute(definition.pattern, handler).catch(() => undefined)
      applied.routes.delete(definition)
    }
    const offline = this.offlineRuns.size > 0
    if (applied.offline !== offline) {
      await context.setOffline(offline)
      applied.offline = offline
    }
  }

  private async reconcileManagedNetworkState(): Promise<void> {
    await Promise.all(
      [...this.contextNetworkState.keys()].map((context) => this.applyManagedNetworkState(context))
    )
  }

  private async cleanupManagedState(): Promise<void> {
    this.routeDefinitions.length = 0
    this.offlineRuns.clear()
    this.stateOwnerRunId = undefined
    if (this.tracing) {
      const tracing = this.tracing
      this.tracing = undefined
      await settleWithin(
        Promise.allSettled([tracing.context.tracing.stop(), tracing.reservation.discard()]),
        CONNECTION_CLOSE_SETTLE_MS
      )
    }
    await settleWithin(
      this.reconcileManagedNetworkState().catch(() => undefined),
      CONNECTION_CLOSE_SETTLE_MS
    )
    for (const [context, applied] of this.contextNetworkState) {
      context.off('close', applied.handleClose)
    }
    this.contextNetworkState.clear()
  }

  private handleManagedContextClosed(context: BrowserContext): void {
    const applied = this.contextNetworkState.get(context)
    if (applied) {
      context.off('close', applied.handleClose)
      this.contextNetworkState.delete(context)
    }
    if (this.tracing?.context !== context) return
    const tracing = this.tracing
    this.tracing = undefined
    this.maybeReleaseStateOwner(tracing.owner.runId)
    void tracing.reservation.discard().catch(() => undefined)
  }

  private async ensureConnected(): Promise<ActiveConnection> {
    if (this.closed) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    // Retirement owns the process-wide detachAutomation boundary. Never attach a fresh official
    // generation until every previously queued close + detach has completed; detachAutomation is
    // intentionally generation-less and could otherwise tear down the replacement connection.
    await this.awaitConnectionLifecycle()
    if (this.closed) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    if (this.connection) {
      if (this.isConnectionUsable(this.connection)) return this.connection
      await this.retireStaleConnection(this.connection)
      return await this.ensureConnected()
    }
    let connecting = this.connecting
    let epoch = this.connectingEpoch
    if (!connecting) {
      epoch = this.connectionEpoch
      connecting = (async (): Promise<ActiveConnection> => {
        const connection = await this.createConnection()
        if (this.closed || this.connectionEpoch !== epoch) {
          // A close/abort raced the asynchronous attach. Retire the late generation through the
          // same global barrier; closing SDK endpoints alone is insufficient because the managed
          // BrowserContext attachment is owned outside the in-memory transport.
          await this.retireStaleConnection(connection)
          throw new ManagedPlaywrightMcpHostError(
            this.closed ? 'mcp.builtin_playwright.closed' : 'mcp.builtin_playwright.protocol_error'
          )
        }
        if (!this.isConnectionUsable(connection)) {
          await this.retireStaleConnection(connection)
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
        }
        this.connection = connection
        return connection
      })()
      this.connecting = connecting
      this.connectingEpoch = epoch
    }
    try {
      return await connecting
    } catch (error) {
      const invalidated = !this.closed && this.connectionEpoch !== epoch
      if (invalidated) {
        if (this.connecting === connecting) {
          this.connecting = undefined
          this.connectingEpoch = undefined
        }
        await this.awaitConnectionLifecycle()
        return await this.ensureConnected()
      }
      throw error
    } finally {
      if (this.connecting === connecting) {
        this.connecting = undefined
        this.connectingEpoch = undefined
      }
    }
  }

  private async createConnection(): Promise<ActiveConnection> {
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair()
    const outputSession = await this.artifactBroker?.openSession()
    const outputDirectory = outputSession?.outputDirectory ?? (await createSecureOutputDirectory())
    const outputLifetime = createManagedConnectionOutputLifetime(
      outputDirectory,
      () => outputSession?.close() ?? removeOutputDirectory(outputDirectory)
    )
    if (this.closed) {
      await (outputSession?.close() ?? removeOutputDirectory(outputDirectory))
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    if (!outputSession) this.outputDirectories.add(outputDirectory)
    let server: ManagedMcpServer | undefined
    let client: ManagedMcpClient | undefined
    let activeConnection: ActiveConnection | undefined
    let resolvedContext: BrowserContext | undefined
    let transportClosed = false
    try {
      server = await this.createOfficialConnection(
        {
          // Approved input files are immutable one-time FileBroker copies. Each file-bearing call
          // copies them into a short-lived 0700 child of this connection's upstream-recognized
          // output root and binds `_meta.cwd` to that exact directory. Unrestricted process-wide
          // file access remains disabled; raw model paths never reach this connection.
          browser: { isolated: false },
          capabilities: [...MANAGED_PLAYWRIGHT_CAPABILITIES],
          codegen: 'none',
          imageResponses: 'omit',
          outputDir: outputDirectory,
          saveSession: false,
          sharedBrowserContext: true,
          timeouts: {
            action: 10_000,
            navigation: 60_000,
            expect: 10_000,
            settle: 500
          }
        },
        async () => {
          const context = await this.getManagedBrowserContext()
          resolvedContext = context
          if (activeConnection) this.bindConnectionContext(activeConnection, context)
          return context
        }
      )
      client = this.createClient()
      const handleTransportClose = (): void => {
        transportClosed = true
        if (activeConnection) this.handleConnectionInvalidated(activeConnection)
      }
      observeManagedProtocolClose(server, handleTransportClose)
      observeManagedProtocolClose(client, handleTransportClose)
      await Promise.all([server.connect(serverTransport), client.connect(clientTransport)])
      if (!outputSession) this.outputDirectories.delete(outputDirectory)
      const connection: ActiveConnection = {
        client,
        server,
        clientTransport,
        serverTransport,
        outputDirectory,
        outputLifetime,
        stale: transportClosed,
        ...(outputSession ? { outputSession } : {})
      }
      activeConnection = connection
      if (resolvedContext) this.bindConnectionContext(connection, resolvedContext)
      return connection
    } catch {
      await settleWithin(
        Promise.allSettled([
          client?.close(),
          server?.close(),
          clientTransport.close(),
          serverTransport.close()
        ]),
        CONNECTION_CLOSE_SETTLE_MS
      )
      await outputLifetime.retireConnection()
      if (!outputSession) this.outputDirectories.delete(outputDirectory)
      // createOfficialConnection may have attached the managed BrowserContext before the MCP
      // handshake failed. No ActiveConnection exists to retire in that case, and no later attach
      // can begin until this shared `connecting` promise settles, so detach the partial attempt
      // here exactly once.
      await this.detachAutomation().catch(() => undefined)
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
  }

  private async ensureOfficialCatalog(
    connection: ActiveConnection,
    signal: AbortSignal
  ): Promise<ReadonlyMap<string, UpstreamToolDescriptor>> {
    if (connection.catalog) return connection.catalog
    if (connection.cataloging) return connection.cataloging
    const cataloging = this.discoverOfficialCatalog(connection, signal)
    connection.cataloging = cataloging
    try {
      const catalog = await cataloging
      connection.catalog = catalog
      return catalog
    } finally {
      if (connection.cataloging === cataloging) connection.cataloging = undefined
    }
  }

  private async discoverOfficialCatalog(
    connection: ActiveConnection,
    signal: AbortSignal
  ): Promise<ReadonlyMap<string, UpstreamToolDescriptor>> {
    const tools: UpstreamToolDescriptor[] = []
    let cursor: string | undefined
    for (let page = 0; page < 8; page += 1) {
      const result = await connection.client.listTools(cursor ? { cursor } : undefined, {
        signal,
        timeout: this.toolTimeoutMs
      })
      tools.push(...result.tools)
      if (tools.length > 256) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
      }
      if (!result.nextCursor) {
        try {
          return validateAndIndexOfficialPlaywrightCatalog(tools)
        } catch {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
        }
      }
      if (result.nextCursor === cursor) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
      }
      cursor = result.nextCursor
    }
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
  }

  private async runBounded<T>(
    operation: (signal: AbortSignal) => Promise<T>,
    callerSignal?: AbortSignal,
    requestedTimeoutMs?: number,
    awaitOperationAbortCleanup = false
  ): Promise<T> {
    if (this.closed) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    if (this.activeCalls.size >= MAX_ACTIVE_CALLS) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
    const controller = new AbortController()
    const timeoutMs = boundedTimeout(requestedTimeoutMs ?? this.toolTimeoutMs)
    const timeout = setTimeout(() => controller.abort('timeout'), timeoutMs)
    const abortFromCaller = (): void => controller.abort('caller')
    callerSignal?.addEventListener('abort', abortFromCaller, { once: true })
    this.activeCalls.add(controller)
    try {
      if (callerSignal?.aborted) controller.abort('caller')
      if (controller.signal.aborted) throw cancellationError(controller.signal.reason)
      const pending = operation(controller.signal)
      const result = await (awaitOperationAbortCleanup
        ? pending
        : raceWithAbort(pending, controller.signal))
      if (controller.signal.aborted) throw cancellationError(controller.signal.reason)
      return result
    } catch (error) {
      if (controller.signal.aborted) {
        const code =
          controller.signal.reason === 'timeout'
            ? 'mcp.builtin_playwright.timeout'
            : 'mcp.builtin_playwright.cancelled'
        throw new ManagedPlaywrightMcpHostError(code)
      }
      throw mapSafeHostError(error)
    } finally {
      clearTimeout(timeout)
      callerSignal?.removeEventListener('abort', abortFromCaller)
      this.activeCalls.delete(controller)
    }
  }

  private async disposeConnection(
    detachAutomation: boolean,
    cancelActiveCalls = true,
    forceDetachWithoutConnection = false
  ): Promise<void> {
    this.connectionEpoch += 1
    if (cancelActiveCalls) {
      for (const controller of this.activeCalls) controller.abort('host_close')
    }
    const connection = this.connection
    this.connection = undefined
    const connecting = this.connecting
    // Queue lifecycle work synchronously, before the first await in this method. A concurrent
    // ensureConnected observes this barrier and cannot attach a replacement generation early.
    const retirement = connection
      ? detachAutomation
        ? this.retireStaleConnection(connection)
        : this.enqueueConnectionLifecycle(async () => this.closeConnection(connection))
      : undefined
    const forcedDetach =
      detachAutomation && forceDetachWithoutConnection && !connection && !connecting
        ? this.enqueueConnectionLifecycle(async () => {
            await this.detachAutomation().catch(() => undefined)
          })
        : undefined
    await this.clearFrameEditorCandidates()
    let connectingSettled = true
    if (connecting) {
      // `ensureConnected` remains the single owner of a connection under construction. Epoch
      // invalidation makes that continuation retire its late value through the lifecycle barrier.
      // Keep the shared promise installed so another ensure cannot start a replacement in parallel.
      connectingSettled = Boolean(await settleWithin(connecting, CONNECTION_CLOSE_SETTLE_MS))
    }
    await Promise.allSettled([
      ...(retirement ? [retirement] : []),
      ...(forcedDetach ? [forcedDetach] : [])
    ])
    await this.awaitConnectionLifecycle()
    // An output directory still present here belongs to an attach attempt that has not completed.
    // Its createConnection continuation remains responsible for cleanup; deleting it underneath
    // the official server could turn a bounded shutdown into a later protocol cascade.
    if (connectingSettled) {
      const orphanedOutputDirectories = [...this.outputDirectories]
      this.outputDirectories.clear()
      await Promise.allSettled(orphanedOutputDirectories.map(removeOutputDirectory))
    }
  }

  private async closeConnection(connection: ActiveConnection): Promise<void> {
    if (connection.context && connection.contextCloseHandler) {
      connection.context.off('close', connection.contextCloseHandler)
      connection.contextCloseHandler = undefined
    }
    connection.stale = true
    connection.closePromise ??= closeConnection(connection)
    await connection.closePromise
  }

  private bindConnectionContext(connection: ActiveConnection, context: BrowserContext): void {
    if (connection.context === context) return
    if (connection.context) {
      connection.stale = true
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    const handleClose = (): void => this.handleConnectionInvalidated(connection)
    connection.context = context
    connection.contextCloseHandler = handleClose
    context.once('close', handleClose)
    if (!this.isConnectionUsable(connection)) handleClose()
  }

  private handleConnectionInvalidated(connection: ActiveConnection): void {
    const wasStale = connection.stale
    connection.stale = true
    if (this.connection === connection) {
      // Clear the reusable reference synchronously. The bounded retirement runs independently so
      // a future serialized call can never re-enter a closed Context or MCP transport while the
      // old SDK endpoints are still settling.
      this.connection = undefined
      this.connectionEpoch += 1
    } else if (wasStale) {
      return
    }
    void this.retireStaleConnection(connection).catch(() => undefined)
  }

  private isConnectionUsable(connection: ActiveConnection): boolean {
    if (connection.stale) return false
    const browser = connection.context?.browser?.()
    return browser === undefined || browser === null || browser.isConnected()
  }

  private async retireStaleConnection(connection: ActiveConnection): Promise<void> {
    connection.stale = true
    if (this.connection === connection) {
      this.connection = undefined
      this.connectionEpoch += 1
    }
    connection.retirementPromise ??= this.enqueueConnectionLifecycle(async () => {
      await this.closeConnection(connection)
      await this.detachAutomation().catch(() => undefined)
    })
    await connection.retirementPromise
  }

  private enqueueConnectionLifecycle(operation: () => Promise<void>): Promise<void> {
    const queued = this.connectionLifecycleTail.then(operation)
    // Keep the shared barrier usable after best-effort close/detach failures while returning the
    // original queued promise to the owner for observability.
    this.connectionLifecycleTail = queued.catch(() => undefined)
    return queued
  }

  private async awaitConnectionLifecycle(): Promise<void> {
    // A close callback may enqueue a retirement while an earlier lifecycle task is settling.
    // Observe until the tail remains stable across an await, not merely until one snapshot ends.
    for (;;) {
      const barrier = this.connectionLifecycleTail
      await barrier
      if (barrier === this.connectionLifecycleTail) return
    }
  }
}

function safeOfficialCallFailureReason(error: unknown): string {
  if (error instanceof ManagedPlaywrightMcpHostError) return error.code
  if (!(error instanceof Error)) return 'unknown_error'
  const message = error.message.toLowerCase()
  if (message.includes('transport') && message.includes('closed')) return 'transport_closed'
  if (message.includes('connection') && message.includes('closed')) return 'connection_closed'
  if (message.includes('target') && message.includes('closed')) return 'target_closed'
  if (message.includes('waitforinitialized')) return 'page_initialization_failed'
  if (message.includes('timed out') || message.includes('timeout')) return 'timeout'
  if (message.includes('protocol')) return 'protocol_error'
  if (message.includes('mcp error')) return 'mcp_error'
  return error.name === 'Error' ? 'unclassified_error' : error.name.toLowerCase()
}

type FixedPlaywrightTargetParser = (
  language: 'javascript',
  locator: string,
  testIdAttributeName?: string
) => string

let fixedPlaywrightTargetParser: FixedPlaywrightTargetParser | undefined

function managedDropTargetLocator(page: Page, target: string): Locator {
  if (/^(f\d+)?e\d+$/u.test(target)) return page.locator(`aria-ref=${target}`)
  const selector = loadFixedPlaywrightTargetParser()('javascript', target, 'data-testid')
  if (!selector) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return page.locator(selector)
}

function loadFixedPlaywrightTargetParser(): FixedPlaywrightTargetParser {
  if (fixedPlaywrightTargetParser) return fixedPlaywrightTargetParser
  // Resolve from @playwright/mcp itself so the selector grammar cannot silently drift to another
  // workspace Playwright version. This is the same parser used by the pinned official 0.0.79 Tab.
  const requireFromHost = createRequire(join(__dirname, 'managed-playwright-core-loader.cjs'))
  const requireFromFixedMcp = createRequire(requireFromHost.resolve('@playwright/mcp'))
  const coreBundle = expectRecordWithoutThrow(
    requireFromFixedMcp('playwright-core/lib/coreBundle') as unknown
  )
  const iso = expectRecordWithoutThrow(coreBundle?.iso)
  const parser = iso?.locatorOrSelectorAsSelector
  if (typeof parser !== 'function') {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
  }
  fixedPlaywrightTargetParser = parser as FixedPlaywrightTargetParser
  return fixedPlaywrightTargetParser
}

async function raceWithAbort<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) throw cancellationError(signal.reason)
  return new Promise<T>((resolve, reject) => {
    const abort = (): void => reject(cancellationError(signal.reason))
    signal.addEventListener('abort', abort, { once: true })
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

function riskFailureResult(failure: BrowserRiskFailure | null): ManagedPlaywrightCallResult {
  if (!failure) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  if (
    failure.dispatchCertainty === 'possibly_dispatched' ||
    failure.code === 'browser.risk_outcome_unknown'
  ) {
    throw new ManagedPlaywrightMcpHostError('browser.risk_outcome_unknown', 'possibly_dispatched')
  }

  let text: string
  switch (failure.code) {
    case 'browser.risk_rejected':
      text = failure.reason
        ? `The user declined this browser destination or operation. Reason: ${failure.reason}`
        : 'The user declined this browser destination or operation.'
      break
    case 'browser.unsupported_host_boundary':
      text = 'This destination crosses a MyCopilot Host isolation boundary and cannot be approved.'
      break
    case 'browser.risk_expired':
      text = 'The browser risk approval expired before the request was dispatched.'
      break
    case 'browser.risk_cancelled':
      text = 'The browser risk approval was cancelled before the request was dispatched.'
      break
    case 'browser.risk_policy_denied':
      text = 'The current browser safety policy does not allow this request.'
      break
    case 'browser.risk_busy':
      text = 'Too many browser risk decisions are currently pending.'
      break
    case 'browser.risk_drift':
      text = 'The browser destination changed while it was being approved, so it was not accessed.'
      break
  }
  return {
    content: [{ type: 'text', text }],
    structuredContent: {
      status: failure.code,
      dispatchCertainty: failure.dispatchCertainty,
      ...(failure.reason ? { rejectionReason: failure.reason } : {})
    },
    isError: true
  }
}

function textToolResult(text: string): ManagedPlaywrightCallResult {
  return { content: [{ type: 'text', text }], isError: false }
}

function managedBrowserConfigResult(): ManagedPlaywrightCallResult {
  return {
    content: [
      {
        type: 'text',
        text: 'Managed browser automation uses the built-in Electron Browser surfaces.'
      }
    ],
    structuredContent: {
      packageName: '@playwright/mcp',
      packageVersion: MANAGED_PLAYWRIGHT_PACKAGE_VERSION,
      browser: 'managed_electron_guest',
      sharedBrowserContext: true,
      imageResponses: 'artifact_only',
      codegen: 'none'
    },
    isError: false
  }
}

function artifactToolResult(
  artifactOrArtifacts: BrowserArtifactReference | readonly BrowserArtifactReference[],
  options: { hostImagePublishPath?: string; readPathUnavailable?: 'too_large' } = {}
): ManagedPlaywrightCallResult {
  const artifacts = Array.isArray(artifactOrArtifacts)
    ? [...artifactOrArtifacts]
    : [artifactOrArtifacts]
  const primary = artifacts[0]
  if (!primary) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  const summary =
    artifacts.length === 1
      ? `Created managed ${primary.kind} Artifact “${primary.displayName}” (${primary.sizeBytes} bytes).`
      : `Created ${artifacts.length} managed browser Artifacts.`
  const text =
    options.readPathUnavailable === 'too_large'
      ? `${summary} This screenshot exceeds the 8 MiB read_image limit, so no readPath is available.`
      : summary
  return {
    content: [{ type: 'text', text }],
    structuredContent: { status: 'completed', artifacts },
    isError: false,
    ...(options.hostImagePublishPath
      ? { hostImagePublishPath: options.hostImagePublishPath }
      : {})
  }
}

function isPdfUnavailableResult(result: ManagedPlaywrightCallResult): boolean {
  return result.content.some(
    (block) => block.type === 'text' && block.text.includes(PDF_UNAVAILABLE_SENTINEL)
  )
}

function pdfUnavailableToolResult(): ManagedPlaywrightCallResult {
  return {
    content: [
      {
        type: 'text',
        text: 'PDF export is unavailable in the current managed Electron browser.'
      }
    ],
    structuredContent: {
      status: 'unavailable',
      code: PDF_UNAVAILABLE_SENTINEL,
      platformScope: 'managed_electron'
    },
    isError: true
  }
}

function withArtifactReferences(
  result: ManagedPlaywrightCallResult,
  artifacts: readonly BrowserArtifactReference[]
): ManagedPlaywrightCallResult {
  if (artifacts.length === 0) return result
  return {
    ...result,
    structuredContent: {
      ...(result.structuredContent ?? {}),
      artifacts: [...artifacts]
    }
  }
}

export async function appendSafeFrameEditorCandidates(
  result: ManagedPlaywrightCallResult,
  context: BrowserContext,
  registerCandidate: (candidate: BrowserFrameEditorCandidate, index: number) => string
): Promise<ManagedPlaywrightCallResult> {
  try {
    const page = context.pages()[0]
    if (!page) return result
    const projection = formatBrowserFrameEditorCandidates(
      await probeBrowserFrameEditors(page),
      registerCandidate
    )
    if (!projection) return result
    return {
      ...result,
      content: [...result.content, { type: 'text', text: projection }]
    }
  } catch {
    // A frame can navigate or detach between the authoritative snapshot and this optional,
    // value-free fallback probe. Never turn a successful upstream snapshot into a failure.
    return result
  }
}

function requiredRunId(authorization: BrowserRiskAuthorizationContext | undefined): string {
  const runId = authorization?.runId
  if (!runId || runId.length > 512) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return runId
}

function expectPositiveDimension(value: unknown): number {
  if (!Number.isSafeInteger(value) || Number(value) < 100 || Number(value) > 8_192) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return Number(value)
}

function managedRouteDefinition(
  runId: string,
  argumentsValue: Readonly<Record<string, unknown>>
): ManagedRouteDefinition {
  const pattern = argumentsValue.pattern
  const status = argumentsValue.status
  const body = argumentsValue.body
  const contentType = argumentsValue.contentType
  const headers = argumentsValue.headers
  const removeHeaders = argumentsValue.removeHeaders
  if (
    typeof pattern !== 'string' ||
    (status !== undefined && (typeof status !== 'number' || !Number.isFinite(status))) ||
    (body !== undefined && typeof body !== 'string') ||
    (contentType !== undefined && typeof contentType !== 'string') ||
    (headers !== undefined &&
      (!Array.isArray(headers) || headers.some((header) => typeof header !== 'string'))) ||
    (removeHeaders !== undefined && typeof removeHeaders !== 'string')
  ) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }

  const addHeaders = headers
    ? Object.fromEntries(
        headers.map((header) => {
          const colonIndex = header.indexOf(':')
          return [header.substring(0, colonIndex).trim(), header.substring(colonIndex + 1).trim()]
        })
      )
    : undefined
  return {
    pattern,
    runId,
    ...(status === undefined ? {} : { status }),
    ...(body === undefined ? {} : { body }),
    ...(contentType === undefined ? {} : { contentType }),
    ...(addHeaders === undefined ? {} : { addHeaders }),
    ...(removeHeaders
      ? { removeHeaders: removeHeaders.split(',').map((header) => header.trim()) }
      : {})
  }
}

function managedRouteListLine(route: ManagedRouteDefinition, index: number): string {
  const details: string[] = []
  if (route.status !== undefined) details.push(`status=${route.status}`)
  if (route.body !== undefined) {
    details.push(
      `body=${route.body.length > 50 ? `${route.body.substring(0, 50)}...` : route.body}`
    )
  }
  if (route.contentType) details.push(`contentType=${route.contentType}`)
  if (route.addHeaders) {
    details.push(`addHeaders=${JSON.stringify(route.addHeaders)}`)
  }
  if (route.removeHeaders) details.push(`removeHeaders=${route.removeHeaders.join(',')}`)
  return `${index + 1}. ${route.pattern}${details.length > 0 ? ` (${details.join(', ')})` : ''}`
}

function artifactSpec(
  toolName: string,
  modelArguments: Record<string, unknown>
):
  | {
      kind: BrowserArtifactKind
      mimeType: string
      suggestedFileName: string
      allowPreview?: boolean
    }
  | undefined {
  const suggested = modelArguments.filename
  const optionalSuggestion = (fallback: string): string => {
    if (suggested === undefined) return fallback
    if (typeof suggested !== 'string') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    return suggested
  }
  switch (toolName) {
    case 'browser_take_screenshot': {
      const explicitType = modelArguments.type
      const suggestedExtension =
        typeof suggested === 'string' ? extname(suggested).toLowerCase() : ''
      const type =
        explicitType === 'jpeg' || explicitType === 'webp' || explicitType === 'png'
          ? explicitType
          : suggestedExtension === '.jpg' || suggestedExtension === '.jpeg'
            ? 'jpeg'
            : suggestedExtension === '.webp'
              ? 'webp'
              : 'png'
      return {
        kind: 'image',
        mimeType: `image/${type}`,
        suggestedFileName: optionalSuggestion(
          `browser-screenshot.${type === 'jpeg' ? 'jpg' : type}`
        )
      }
    }
    case 'browser_pdf_save':
      return {
        kind: 'pdf',
        mimeType: 'application/pdf',
        suggestedFileName: optionalSuggestion('browser-page.pdf')
      }
    case 'browser_snapshot':
      return suggested === undefined
        ? undefined
        : {
            kind: 'snapshot',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-snapshot.yml')
          }
    case 'browser_console_messages':
      return suggested === undefined
        ? undefined
        : {
            kind: 'console',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-console.log'),
            allowPreview: false
          }
    case 'browser_network_requests':
      return suggested === undefined
        ? undefined
        : {
            kind: 'network',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-network.log'),
            allowPreview: false
          }
    case 'browser_network_request':
      return suggested === undefined
        ? undefined
        : {
            kind: 'network',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-network-detail.txt'),
            allowPreview: false
          }
    case 'browser_evaluate':
      return suggested === undefined
        ? undefined
        : {
            kind: 'text',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-evaluate-result.txt'),
            allowPreview: false
          }
    case 'browser_storage_state':
      return {
        kind: 'json',
        mimeType: 'application/json',
        suggestedFileName: optionalSuggestion('browser-storage-state.json'),
        allowPreview: false
      }
    default:
      return undefined
  }
}

async function settleWithin<T>(
  operation: Promise<T>,
  timeoutMs: number
): Promise<PromiseSettledResult<T> | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<undefined>((resolve) => {
    timer = setTimeout(() => resolve(undefined), timeoutMs)
  })
  const settled = operation.then<PromiseSettledResult<T>, PromiseSettledResult<T>>(
    (value) => ({ status: 'fulfilled', value }),
    (reason: unknown) => ({ status: 'rejected', reason })
  )
  try {
    return await Promise.race([settled, timeout])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

function expectArgumentRecord(value: unknown): Record<string, unknown> {
  const record = expectRecordWithoutThrow(value)
  if (!record) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return record
}

function stripHostArguments(record: Record<string, unknown>): Record<string, unknown> {
  const copy = { ...record }
  delete copy.call_reason
  return copy
}

type ToolSurfaceLeaseMode = 'creating' | 'existing' | 'optional_existing' | 'none'

function toolSurfaceLeaseMode(
  name: string,
  modelArguments: Readonly<Record<string, unknown>>
): ToolSurfaceLeaseMode {
  // On an empty BrowserContext, fixed Playwright implements browser_navigate by creating its
  // first Page. Do not precreate an unauthorised blank Surface through beginToolSurfaceLease();
  // an absent existing lease must reach executeManagedTargetCreatingTool so Target.createTarget
  // is covered by the request's one-shot Main authority.
  if (name === 'browser_navigate') return 'optional_existing'
  if (name === 'browser_tabs') {
    if (modelArguments.action === 'list') return 'optional_existing'
    if (modelArguments.action === 'close' && modelArguments.index === undefined) return 'existing'
    return 'none'
  }
  if (CREATING_PAGE_TOOLS.has(name)) return 'creating'
  if (EXISTING_PAGE_TOOLS.has(name)) return 'existing'
  return 'none'
}

function shouldSynchronizeOfficialSurface(
  name: string,
  modelArguments: Readonly<Record<string, unknown>>
): boolean {
  if (name === 'browser_close') return false
  if (name === 'browser_tabs') return modelArguments.action === 'list'
  return toolSurfaceLeaseMode(name, modelArguments) !== 'none'
}

function targetCreationIntentForTool(
  name: string,
  modelArguments: Readonly<Record<string, unknown>>,
  hasSurfaceLease: boolean
): 'background' | 'interactive' | undefined {
  if (name === 'browser_tabs' && modelArguments.action === 'new') return 'interactive'
  if (name === 'browser_tabs' && modelArguments.action === 'list' && !hasSurfaceLease) {
    return 'interactive'
  }
  if (name === 'browser_storage_state' || name === 'browser_set_storage_state') return 'background'
  return undefined
}

function toolUsesManagedPageNetworkOperation(
  name: string,
  modelArguments: Readonly<Record<string, unknown>>,
  hasSurfaceLease: boolean,
  hasManagedSurfaceLeasing: boolean
): boolean {
  return (
    ((hasSurfaceLease || !hasManagedSurfaceLeasing) &&
      shouldSynchronizeOfficialSurface(name, modelArguments)) ||
    (name === 'browser_tabs' &&
      (modelArguments.action === 'select' || modelArguments.action === 'close'))
  )
}

function validateReviewedArguments(
  tool: ManagedPlaywrightToolManifestEntry,
  value: Record<string, unknown>
): void {
  if (Buffer.byteLength(JSON.stringify(value), 'utf8') > MAX_ARGUMENT_BYTES) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  if (
    !matchesBoundedJsonSchema(value, tool.inputSchema, {
      maxDepth: MAX_ARGUMENT_DEPTH,
      maxNodes: MAX_ARGUMENT_NODES,
      maxArrayItems: 256,
      maxObjectProperties: 256
    })
  ) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
}

function parseBoundedToolResult(value: unknown): ManagedPlaywrightCallResult {
  const result = expectRecord(value)
  if (!Array.isArray(result.content) || result.content.length > MAX_CONTENT_BLOCKS) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
  }
  const contentBudget = new ContentBlockBudget()
  let totalEncodedMediaBytes = 0
  const content = result.content.map((block) => {
    const parsed = parseContentBlock(block, contentBudget)
    const encodedMedia = encodedMediaData(parsed)
    if (encodedMedia !== undefined) {
      const bytes = Buffer.byteLength(encodedMedia, 'utf8')
      if (bytes > MAX_ENCODED_MEDIA_BYTES) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
      }
      totalEncodedMediaBytes += bytes
    }
    return parsed
  })
  if (totalEncodedMediaBytes > MAX_TOTAL_ENCODED_MEDIA_BYTES) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
  }
  const structuredContent = result.structuredContent
  if (
    structuredContent !== undefined &&
    (structuredContent === null ||
      typeof structuredContent !== 'object' ||
      Array.isArray(structuredContent))
  ) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  const bounded = {
    content,
    ...(structuredContent === undefined
      ? {}
      : { structuredContent: cloneBoundedStructuredContent(structuredContent) }),
    isError: result.isError === true
  }
  if (Buffer.byteLength(JSON.stringify(bounded), 'utf8') > MAX_RESULT_BYTES) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
  }
  return bounded
}

function adaptReviewedToolResult(
  toolName: string,
  result: ManagedPlaywrightCallResult
): ManagedPlaywrightCallResult {
  const frameFailure = safeFrameFailureResult(toolName, result)
  if (frameFailure) return frameFailure
  return result
}

type ManagedFrameFailureCode =
  | 'frame_not_found'
  | 'stale_frame_ref'
  | 'frame_not_editable'
  | 'frame_input_delivery_failed'
  | 'frame_detached'
  | 'target_closed'

class ManagedFrameEditorTargetError extends Error {
  readonly name = 'ManagedFrameEditorTargetError'

  constructor(readonly code: ManagedFrameFailureCode) {
    super(code)
  }
}

function managedFrameFailureResult(code: ManagedFrameFailureCode): ManagedPlaywrightCallResult {
  return {
    content: [{ type: 'text', text: `Managed browser frame operation failed: ${code}.` }],
    structuredContent: { status: 'failed', errorCode: code, contentOmitted: true },
    isError: true
  }
}

function safeFrameFailureResult(
  toolName: string,
  result: ManagedPlaywrightCallResult
): ManagedPlaywrightCallResult | undefined {
  if (!result.isError || !FRAME_INTERACTION_TOOLS.has(toolName)) return undefined
  const diagnostic = result.content
    .flatMap((block) => (block.type === 'text' ? [block.text] : []))
    .join('\n')
  const code = classifyFrameFailure(diagnostic)
  if (!code) return undefined
  return managedFrameFailureResult(code)
}

function classifyFrameFailure(diagnostic: string): ManagedFrameFailureCode | undefined {
  if (/frame_input_delivery_failed/iu.test(diagnostic)) return 'frame_input_delivery_failed'
  if (/ref\s+\S+\s+not found in the current page snapshot/iu.test(diagnostic)) {
    return 'stale_frame_ref'
  }
  if (/frame (?:was |has been )?detached|detached frame/iu.test(diagnostic)) {
    return 'frame_detached'
  }
  if (
    /target (?:page, context or browser )?(?:has been )?closed|target_closed/iu.test(diagnostic)
  ) {
    return 'target_closed'
  }
  if (/not editable|not an (?:input|textarea|select)|element is not enabled/iu.test(diagnostic)) {
    return 'frame_not_editable'
  }
  if (/does not match any elements|frame[^\n]*not found/iu.test(diagnostic)) {
    return 'frame_not_found'
  }
  return undefined
}

const TERMINAL_MANAGED_CONNECTION_PATTERN =
  /(?:target page, context or browser|page|browser|connection|session|transport) (?:has been |was |is )?closed|target_closed|transportclosed|protocol error[^\n]*session closed/iu

function isTerminalManagedConnectionResult(result: ManagedPlaywrightCallResult): boolean {
  if (!result.isError) return false
  if (result.structuredContent?.errorCode === 'target_closed') return true
  const diagnostic = result.content
    .flatMap((block) => (block.type === 'text' ? [block.text] : []))
    .join('\n')
  return TERMINAL_MANAGED_CONNECTION_PATTERN.test(diagnostic)
}

const FRAME_INTERACTION_TOOLS = new Set([
  'browser_click',
  'browser_fill_form',
  'browser_press_key',
  'browser_type'
])

function cloneBoundedStructuredContent(value: object): Record<string, unknown> {
  let nodes = 0
  let encodedBytes = 0
  const ancestors = new WeakSet<object>()
  const addBytes = (amount: number): void => {
    encodedBytes += amount
    if (encodedBytes > MAX_STRUCTURED_CONTENT_BYTES) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
    }
  }
  const clone = (current: unknown, depth: number): unknown => {
    nodes += 1
    if (depth > MAX_ARGUMENT_DEPTH || nodes > MAX_ARGUMENT_NODES) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
    }
    if (current === null) {
      addBytes(4)
      return null
    }
    if (typeof current === 'string') {
      const bytes = Buffer.byteLength(current, 'utf8')
      if (bytes > MAX_STRUCTURED_STRING_BYTES) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
      }
      addBytes(Buffer.byteLength(JSON.stringify(current), 'utf8'))
      return current
    }
    if (typeof current === 'boolean') {
      addBytes(current ? 4 : 5)
      return current
    }
    if (typeof current === 'number') {
      if (!Number.isFinite(current)) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
      }
      addBytes(String(current).length)
      return current
    }
    if (typeof current !== 'object') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    if (ancestors.has(current)) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    ancestors.add(current)
    try {
      if (Array.isArray(current)) {
        if (current.length > MAX_STRUCTURED_PROPERTIES) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
        }
        addBytes(2 + Math.max(0, current.length - 1))
        return current.map((child) => clone(child, depth + 1))
      }
      const prototype = Object.getPrototypeOf(current)
      if (prototype !== Object.prototype && prototype !== null) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
      }
      const record = current as Record<string, unknown>
      const keys = Object.keys(record)
      if (keys.length > MAX_STRUCTURED_PROPERTIES) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
      }
      addBytes(2 + Math.max(0, keys.length - 1))
      const copy: Record<string, unknown> = {}
      for (const key of keys) {
        if (Buffer.byteLength(key, 'utf8') > MAX_STRUCTURED_STRING_BYTES) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
        }
        addBytes(Buffer.byteLength(JSON.stringify(key), 'utf8') + 1)
        copy[key] = clone(record[key], depth + 1)
      }
      return copy
    } finally {
      ancestors.delete(current)
    }
  }

  const cloned = clone(value, 0)
  if (!cloned || typeof cloned !== 'object' || Array.isArray(cloned)) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  return cloned as Record<string, unknown>
}

function encodedMediaData(block: ManagedPlaywrightContentBlock): string | undefined {
  if (block.type === 'image' || block.type === 'audio') return block.data
  if (block.type === 'embedded_resource' && block.resource.contentType === 'blob') {
    return block.resource.data
  }
  return undefined
}

class ContentBlockBudget {
  private encodedStringBytes = 0

  addString(value: string, fieldLimit: number): void {
    if (Buffer.byteLength(value, 'utf8') > fieldLimit) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
    }
    this.encodedStringBytes += jsonEncodedStringBytes(value)
    if (this.encodedStringBytes > MAX_RESULT_BYTES) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
    }
  }
}

function jsonEncodedStringBytes(value: string): number {
  // Include the surrounding JSON quotes. Counting avoids allocating an escaped copy before the
  // aggregate result budget has been enforced.
  let bytes = 2
  for (let index = 0; index < value.length;) {
    const codePoint = value.codePointAt(index)
    if (codePoint === undefined) break
    const width = codePoint > 0xffff ? 2 : 1
    const isLoneSurrogate = width === 1 && codePoint >= 0xd800 && codePoint <= 0xdfff
    if (
      codePoint === 0x22 ||
      codePoint === 0x5c ||
      codePoint === 0x08 ||
      codePoint === 0x09 ||
      codePoint === 0x0a ||
      codePoint === 0x0c ||
      codePoint === 0x0d
    ) {
      bytes += 2
    } else if (codePoint < 0x20 || isLoneSurrogate) {
      bytes += 6
    } else if (codePoint <= 0x7f) {
      bytes += 1
    } else if (codePoint <= 0x7ff) {
      bytes += 2
    } else if (codePoint <= 0xffff) {
      bytes += 3
    } else {
      bytes += 4
    }
    index += width
  }
  return bytes
}

function parseContentBlock(
  value: unknown,
  budget: ContentBlockBudget
): ManagedPlaywrightContentBlock {
  const block = expectRecord(value)
  if (block.type === 'text' && typeof block.text === 'string') {
    budget.addString(block.text, MAX_TEXT_BYTES)
    return { type: 'text', text: block.text }
  }
  if (
    (block.type === 'image' || block.type === 'audio') &&
    typeof block.data === 'string' &&
    typeof block.mimeType === 'string'
  ) {
    budget.addString(block.data, MAX_ENCODED_MEDIA_BYTES)
    budget.addString(block.mimeType, MAX_RESOURCE_MIME_TYPE_BYTES)
    return { type: block.type, data: block.data, mime_type: block.mimeType }
  }
  if (block.type === 'resource') {
    const resource = expectRecord(block.resource)
    if (typeof resource.uri !== 'string') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    budget.addString(resource.uri, MAX_RESOURCE_URI_BYTES)
    if (typeof resource.mimeType === 'string') {
      budget.addString(resource.mimeType, MAX_RESOURCE_MIME_TYPE_BYTES)
    }
    if (typeof resource.text === 'string') {
      budget.addString(resource.text, MAX_RESOURCE_TEXT_BYTES)
      return {
        type: 'embedded_resource',
        resource: {
          contentType: 'text',
          uri: resource.uri,
          text: resource.text,
          ...(typeof resource.mimeType === 'string' ? { mime_type: resource.mimeType } : {})
        }
      }
    }
    if (typeof resource.blob === 'string') {
      budget.addString(resource.blob, MAX_ENCODED_MEDIA_BYTES)
      return {
        type: 'embedded_resource',
        resource: {
          contentType: 'blob',
          uri: resource.uri,
          data: resource.blob,
          ...(typeof resource.mimeType === 'string' ? { mime_type: resource.mimeType } : {})
        }
      }
    }
  }
  if (
    block.type === 'resource_link' &&
    typeof block.uri === 'string' &&
    typeof block.name === 'string'
  ) {
    budget.addString(block.uri, MAX_RESOURCE_URI_BYTES)
    budget.addString(block.name, MAX_RESOURCE_NAME_BYTES)
    if (typeof block.title === 'string') {
      budget.addString(block.title, MAX_RESOURCE_TITLE_BYTES)
    }
    if (typeof block.description === 'string') {
      budget.addString(block.description, MAX_RESOURCE_DESCRIPTION_BYTES)
    }
    if (typeof block.mimeType === 'string') {
      budget.addString(block.mimeType, MAX_RESOURCE_MIME_TYPE_BYTES)
    }
    return {
      type: 'resource_link',
      resource: {
        uri: block.uri,
        name: block.name,
        ...(typeof block.title === 'string' ? { title: block.title } : {}),
        ...(typeof block.description === 'string' ? { description: block.description } : {}),
        ...(typeof block.mimeType === 'string' ? { mimeType: block.mimeType } : {}),
        ...(typeof block.size === 'number' && Number.isSafeInteger(block.size) && block.size >= 0
          ? { size: block.size }
          : {})
      }
    }
  }
  throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
}

async function closeConnection(connection: ActiveConnection): Promise<void> {
  await settleWithin(
    Promise.allSettled([
      connection.client.close(),
      connection.server.close(),
      connection.clientTransport.close(),
      connection.serverTransport.close()
    ]),
    CONNECTION_CLOSE_SETTLE_MS
  )
  await connection.outputLifetime.retireConnection()
}

function observeManagedProtocolClose(peer: { onclose?: () => void }, observer: () => void): void {
  const previous = peer.onclose
  peer.onclose = () => {
    try {
      previous?.()
    } finally {
      observer()
    }
  }
}

function createManagedConnectionOutputLifetime(
  outputDirectory: string,
  closeBase: () => Promise<void>
): ManagedConnectionOutputLifetime {
  const retainedDirectories = new Set<string>()
  let closeRequested = false
  let baseClose: Promise<void> | undefined
  const maybeCloseBase = (): Promise<void> => {
    if (!closeRequested || retainedDirectories.size > 0) return Promise.resolve()
    return (baseClose ??= Promise.resolve().then(closeBase))
  }
  return {
    retainDirectory: (directory) => {
      if (
        closeRequested ||
        dirname(directory) !== outputDirectory ||
        !basename(directory).startsWith('.file-input-')
      ) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
      retainedDirectories.add(directory)
      return onceAsync(async () => {
        await rm(directory, { force: true, recursive: true }).catch(() => undefined)
        retainedDirectories.delete(directory)
        await maybeCloseBase()
      })
    },
    retireConnection: async () => {
      closeRequested = true
      await maybeCloseBase()
    }
  }
}

async function createSecureOutputDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-playwright-mcp-'))
  // `mkdtemp` is already private on POSIX. Enforce the intended boundary explicitly so a
  // permissive process umask or future platform implementation cannot widen access.
  await chmod(directory, 0o700)
  return directory
}

async function removeOutputDirectory(directory: string): Promise<void> {
  await rm(directory, { force: true, recursive: true }).catch(() => undefined)
}

function onceAsync(operation: () => Promise<void>): () => Promise<void> {
  let pending: Promise<void> | undefined
  return () => (pending ??= Promise.resolve().then(operation))
}

function boundedTimeout(value: number | undefined): number {
  if (value === undefined) return DEFAULT_TOOL_TIMEOUT_MS
  if (!Number.isSafeInteger(value) || value <= 0) return DEFAULT_TOOL_TIMEOUT_MS
  return Math.min(value, MAX_TOOL_TIMEOUT_MS)
}

function mapSafeHostError(error: unknown): ManagedPlaywrightMcpHostError {
  if (error instanceof ManagedPlaywrightMcpHostError) return error
  if (
    error instanceof ManagedPlaywrightSensitiveGrantError ||
    error instanceof BrowserFileBrokerError
  ) {
    return new ManagedPlaywrightMcpHostError(
      error instanceof ManagedPlaywrightSensitiveGrantError
        ? (`mcp.builtin_playwright.sensitive_grant_${error.code}` as const)
        : error.code === 'browser.file.capacity'
          ? 'mcp.builtin_playwright.busy'
          : error.code === 'browser.file.too_large'
            ? 'mcp.builtin_playwright.output_too_large'
            : 'mcp.builtin_playwright.invalid_arguments',
      'definitely_not_dispatched'
    )
  }
  if (error instanceof BrowserDownloadBrokerError) {
    switch (error.code) {
      case 'browser.download.cancelled':
        return new ManagedPlaywrightMcpHostError(
          'mcp.builtin_playwright.cancelled',
          error.dispatchCertainty
        )
      case 'browser.download.target_closed':
        return new ManagedPlaywrightMcpHostError('browser.target_closed', error.dispatchCertainty)
      case 'browser.download.too_large':
        return new ManagedPlaywrightMcpHostError(
          'mcp.builtin_playwright.output_too_large',
          error.dispatchCertainty
        )
      case 'browser.download.busy':
        return new ManagedPlaywrightMcpHostError(
          'mcp.builtin_playwright.busy',
          error.dispatchCertainty
        )
      case 'browser.download.interrupted':
      case 'browser.download.outcome_unknown':
        return new ManagedPlaywrightMcpHostError(
          error.dispatchCertainty === 'possibly_dispatched'
            ? 'browser.risk_outcome_unknown'
            : 'mcp.builtin_playwright.protocol_error',
          error.dispatchCertainty
        )
      case 'browser.download.artifact_failed':
      case 'browser.download.closed':
        return new ManagedPlaywrightMcpHostError(
          'mcp.builtin_playwright.protocol_error',
          error.dispatchCertainty
        )
    }
  }
  if (error instanceof BrowserArtifactBrokerError) {
    if (error.code === 'browser.artifact.invalid_name') {
      return new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    if (error.code === 'browser.artifact.too_large') {
      return new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.output_too_large')
    }
    if (error.code === 'browser.artifact.capacity') {
      return new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
    return new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  if (error !== null && typeof error === 'object' && 'code' in error) {
    const code = (error as { code?: unknown }).code
    if (
      code === 'browser.surface_unavailable' ||
      code === 'browser.surface_capacity_exceeded' ||
      code === 'browser.target_closed'
    ) {
      return new ManagedPlaywrightMcpHostError(
        code,
        code === 'browser.surface_capacity_exceeded' ? 'definitely_not_dispatched' : undefined
      )
    }
  }
  return new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
}

function cancellationError(reason: unknown): ManagedPlaywrightMcpHostError {
  return new ManagedPlaywrightMcpHostError(
    reason === 'timeout' ? 'mcp.builtin_playwright.timeout' : 'mcp.builtin_playwright.cancelled'
  )
}

function expectRecord(value: unknown): Record<string, unknown> {
  const record = expectRecordWithoutThrow(value)
  if (!record) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  return record
}

function expectRecordWithoutThrow(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}

export const MANAGED_PLAYWRIGHT_RUNTIME_VERSION = MANAGED_PLAYWRIGHT_PACKAGE_VERSION
