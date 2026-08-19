import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import type { BrowserContext, ElementHandle, Frame, Route } from 'playwright'
import type { BrowserArtifactKind, BrowserArtifactReference } from '@mycopilot/protocol'
import { createConnection } from '@playwright/mcp'
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { tmpdir } from 'node:os'
import { extname, join } from 'node:path'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
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
  type BrowserFileReadLease
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
  resizeActiveSurface?(input: { width: number; height: number }): Promise<{
    width: number
    height: number
  }>
  printActiveSurfaceToPdf?(): Promise<Uint8Array>
}

export type ManagedPlaywrightConnectionFactory = (
  config: Parameters<typeof createConnection>[0],
  contextGetter: () => Promise<BrowserContext>
) => Promise<ManagedMcpServer>

interface ManagedMcpServer {
  connect(transport: InMemoryTransport): Promise<void>
  close(): Promise<void>
}

export interface ManagedMcpClient {
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
  outputSession?: BrowserArtifactOutputSession
  catalog?: ReadonlyMap<string, UpstreamToolDescriptor>
  cataloging?: Promise<ReadonlyMap<string, UpstreamToolDescriptor>>
}

interface ManagedRouteDefinition {
  pattern: string
  runId: string
  status: number
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
  signal: AbortSignal
  markDispatched(): void
}

interface PreparedSensitiveGrant {
  readonly lease: ManagedPlaywrightSensitiveGrantLease
  readonly targetBinding: ManagedPlaywrightSensitiveTargetBindingLease
  readonly target: ManagedPlaywrightSensitiveTargetIdentity
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
  private readonly closeSurface: () => Promise<void>
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
  private connectionEpoch = 0
  private closed = false
  private stateOwnerRunId?: string

  constructor(options: ManagedPlaywrightMcpHostOptions) {
    this.artifactBroker = options.artifactBroker
    this.beginNetworkOperation = options.beginNetworkOperation
    this.getBrowserContext = options.getBrowserContext
    this.getActiveSurfaceIdentity = options.getActiveSurfaceIdentity
    this.fileBroker = options.fileBroker
    this.finalizeBrowserRun = options.finalizeBrowserRun
    this.releaseBrowserCapability = options.releaseBrowserCapability
    this.releaseBrowserToolCall = options.releaseBrowserToolCall
    this.surfaceGroup = options.surfaceGroup
    this.sensitiveTargetBindings = options.sensitiveTargetBindings
    this.closeSurface = options.closeSurface
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
      await this.disposeConnection(true)
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
    try {
      return await this.runBounded(
        async (operationSignal) =>
          this.serializeDispatch(operationSignal, async () => {
            this.assertRunStateAccess(options.authorizationContext?.runId)
            const preDispatchResult = await this.executeSensitivePreDispatchAdapter({
              name,
              modelArguments,
              options,
              signal: operationSignal
            })
            if (preDispatchResult) {
              responseReceived = true
              return preDispatchResult
            }

            let sensitiveGrant: PreparedSensitiveGrant | undefined
            let fileLease: BrowserFileReadLease | undefined
            const outcome = await (async (): Promise<ManagedPlaywrightCallResult> => {
              // Approval revalidation is a pure pre-dispatch gate. A missing, expired, or drifted
              // grant must not attach automation, create a Surface, or start the managed MCP.
              sensitiveGrant = await this.prepareSensitiveGrant(
                reviewed,
                modelArguments,
                options.authorizationContext
              )
              const connection = await this.ensureConnected()
              await this.ensureOfficialCatalog(connection, operationSignal)
              const preparedFiles = await this.prepareSensitiveFiles({
                name,
                arguments: serverArguments,
                authorizationContext: options.authorizationContext
              })
              serverArguments = preparedFiles.arguments
              fileLease = preparedFiles.lease
              const markDispatched = (): void => {
                if (sensitiveGrant) {
                  this.assertSensitiveDispatchTarget(sensitiveGrant.target)
                  sensitiveGrant.targetBinding.markDispatched()
                  sensitiveGrant.lease.markDispatched()
                }
                dispatchStarted = true
              }
              const hostAdapted = await this.executeHostAdapter({
                name,
                arguments: serverArguments,
                modelArguments,
                options,
                connection,
                signal: operationSignal,
                markDispatched
              })
              if (hostAdapted) {
                responseReceived = true
                return hostAdapted
              }

              let riskLease: BrowserNetworkOperationLease | undefined
              let artifactPlan: ManagedUpstreamArtifactPlan | undefined
              let artifactCommitted = false
              try {
                if (this.beginNetworkOperation) {
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

                let rawResult: unknown
                try {
                  artifactPlan = await this.prepareUpstreamArtifactPlan({
                    name,
                    arguments: serverArguments,
                    modelArguments,
                    options,
                    connection
                  })
                  // Once the official MCP handler receives the call, page script, navigation, or form
                  // submission may already have happened. Later network refusals are therefore never
                  // represented as a safe pre-dispatch denial and must not be replayed automatically.
                  riskLease?.markDispatched()
                  markDispatched()
                  rawResult = await connection.client.callTool(
                    { name, arguments: artifactPlan?.serverArguments ?? serverArguments },
                    undefined,
                    {
                      signal: operationSignal,
                      timeout: boundedTimeout(options.timeoutMs ?? this.toolTimeoutMs),
                      resetTimeoutOnProgress: false
                    }
                  )
                  responseReceived = true
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
                let parsed = adaptReviewedToolResult(
                  name,
                  parseBoundedToolResult(rawResult, isSafeDiagnosticTool(name)),
                  modelArguments
                )
                if (name === 'browser_snapshot' && !artifactPlan && !parsed.isError) {
                  try {
                    parsed = await this.appendSafeFrameEditorCandidates(
                      parsed,
                      await this.getManagedBrowserContext(),
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
                return artifactToolResult([artifact, ...downloadArtifacts])
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
            if (!outcome.ok) throw outcome.error
            if (targetFenceError) throw targetFenceError
            return outcome.result
          }),
        options.signal,
        options.timeoutMs
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
      return await operation()
    } finally {
      release()
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

  private async executeSensitivePreDispatchAdapter(input: {
    name: string
    modelArguments: Record<string, unknown>
    options: HostAdapterInput['options']
    signal: AbortSignal
  }): Promise<ManagedPlaywrightCallResult | undefined> {
    if (input.name !== 'browser_file_upload' || input.modelArguments.paths !== undefined) {
      return undefined
    }
    if (input.options.authorizationContext?.builtinToolGrant) {
      throw new ManagedPlaywrightSensitiveGrantError('drifted')
    }
    if (!this.fileBroker) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    if (input.signal.aborted) throw cancellationError(input.signal.reason)
    const references = await this.fileBroker.selectForRead({
      owner: this.fileOwner(input.options.authorizationContext),
      multiple: true
    })
    if (input.signal.aborted) throw cancellationError(input.signal.reason)
    if (references.length === 0) {
      return {
        content: [
          {
            type: 'text',
            text: 'No file was selected. The page was not changed.'
          }
        ],
        structuredContent: {
          schemaVersion: 1,
          status: 'file_selection_cancelled',
          contentOmitted: true
        },
        isError: false
      }
    }
    return {
      content: [
        {
          type: 'text',
          text: references
            .map(
              (reference, index) =>
                `${index + 1}. ${reference.handle} (${reference.displayName}, ${reference.sizeBytes} bytes)`
            )
            .join('\n')
        }
      ],
      structuredContent: {
        schemaVersion: 1,
        status: 'file_selection_ready',
        contentOmitted: true,
        fileCount: references.length
      },
      isError: false
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
        activeOrigin: '',
        consumedGrantIds: this.consumedSensitiveGrantIds
      })
      return undefined
    }
    assertSensitiveTopFrameScope(reviewed.rawName, modelArguments)
    const target = this.getActiveBrowserTarget()
    const lease = validateSensitiveToolGrant({
      reviewed,
      modelArguments,
      authorizationContext,
      activeOrigin: target.origin,
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
    if (
      target.surfaceId !== targetBinding.target.surfaceId ||
      target.generation !== targetBinding.target.generation ||
      target.navigationEpoch !== targetBinding.target.navigationEpoch ||
      target.origin !== targetBinding.target.origin
    ) {
      throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
    }
    return { lease, target, targetBinding }
  }

  private async prepareSensitiveFiles(input: {
    name: string
    arguments: Record<string, unknown>
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  }): Promise<{ arguments: Record<string, unknown>; lease?: BrowserFileReadLease }> {
    let handles: readonly string[] | undefined
    if (input.name === 'browser_file_upload' || input.name === 'browser_drop') {
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
      if (handles.length === 0) return { arguments: input.arguments }
    } else if (input.name === 'browser_set_storage_state') {
      if (typeof input.arguments.filename !== 'string') {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
      handles = [input.arguments.filename]
    } else {
      return { arguments: input.arguments }
    }
    if (!this.fileBroker) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    const lease = await this.fileBroker.consumeForRead({
      owner: this.fileOwner(input.authorizationContext),
      handles
    })
    return {
      arguments:
        input.name === 'browser_set_storage_state'
          ? { ...input.arguments, filename: lease.paths[0] }
          : { ...input.arguments, paths: [...lease.paths] },
      lease
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
    context: BrowserContext,
    authorizationContext: BrowserRiskAuthorizationContext | undefined
  ): Promise<ManagedPlaywrightCallResult> {
    await this.clearFrameEditorCandidates()
    const page = context.pages()[0]
    if (!page) return result
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
      case 'browser_close': {
        input.markDispatched()
        await this.cleanupManagedState()
        await this.closeSurface()
        await this.disposeConnection(false, false)
        return textToolResult('The managed browser tab was closed.')
      }
      case 'browser_get_config':
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
      case 'browser_tabs':
        return this.executeTabsAdapter(input)
      case 'browser_resize': {
        const width = expectPositiveDimension(input.arguments.width)
        const height = expectPositiveDimension(input.arguments.height)
        if (!this.surfaceGroup?.resizeActiveSurface) {
          throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
        }
        await this.surfaceGroup.ensureActiveSurface()
        input.markDispatched()
        const actual = await this.surfaceGroup.resizeActiveSurface({ width, height })
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
      case 'browser_pdf_save': {
        if (!this.surfaceGroup?.printActiveSurfaceToPdf || !this.artifactBroker) {
          throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
        }
        await this.getManagedBrowserContext()
        const spec = artifactSpec(input.name, input.modelArguments)
        if (!spec) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        const owner = this.artifactOwner(input.options)
        input.markDispatched()
        const bytes = await this.surfaceGroup.printActiveSurfaceToPdf()
        if (input.signal.aborted) throw cancellationError(input.signal.reason)
        const artifact = await this.artifactBroker.storeBytes({
          owner,
          kind: spec.kind,
          mimeType: spec.mimeType,
          suggestedFileName: spec.suggestedFileName,
          bytes
        })
        if (input.signal.aborted) throw cancellationError(input.signal.reason)
        return artifactToolResult(artifact)
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
        input.markDispatched()
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
        const pattern = input.arguments.pattern
        const status = input.arguments.status ?? 200
        if (
          typeof pattern !== 'string' ||
          pattern.length < 1 ||
          pattern.length > 2_048 ||
          !Number.isSafeInteger(status) ||
          Number(status) < 100 ||
          Number(status) > 599
        ) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        await this.getManagedBrowserContext()
        this.claimStateOwner(runId)
        input.markDispatched()
        const definition = { pattern, runId, status: Number(status) }
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
        return textToolResult(
          `A managed response route was added for ${safeRoutePattern(pattern)}.`
        )
      }
      case 'browser_route_list': {
        const runId = requiredRunId(input.options.authorizationContext)
        await this.getManagedBrowserContext()
        const routes = this.routeDefinitions.filter((route) => route.runId === runId)
        return {
          content: [
            {
              type: 'text',
              text:
                routes.length === 0
                  ? 'No active managed routes.'
                  : routes
                      .map(
                        (route, index) =>
                          `${index + 1}. ${safeRoutePattern(route.pattern)} (status=${route.status})`
                      )
                      .join('\n')
            }
          ],
          structuredContent: {
            routes: routes.map((route) => ({
              pattern: safeRoutePattern(route.pattern),
              status: route.status
            }))
          },
          isError: false
        }
      }
      case 'browser_unroute': {
        const runId = requiredRunId(input.options.authorizationContext)
        const pattern = input.arguments.pattern
        if (pattern !== undefined && (typeof pattern !== 'string' || pattern.length > 2_048)) {
          throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
        }
        await this.getManagedBrowserContext()
        input.markDispatched()
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
        return textToolResult(`Removed ${removed} managed route${removed === 1 ? '' : 's'}.`)
      }
      case 'browser_start_tracing':
        return this.startTracing(input)
      case 'browser_stop_tracing':
        return this.stopTracing(input)
      default:
        return undefined
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

    input.markDispatched()
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

  private async executeTabsAdapter(input: HostAdapterInput): Promise<ManagedPlaywrightCallResult> {
    if (!this.surfaceGroup) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
    const action = input.arguments.action
    if (action === 'list') {
      await this.surfaceGroup.ensureActiveSurface()
      return tabsToolResult(this.surfaceGroup.listSurfaces())
    }
    if (this.stateOwnerRunId) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.busy')
    }
    if (action === 'new') {
      const url = input.arguments.url
      if (url !== undefined && typeof url !== 'string') {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
      input.markDispatched()
      await this.surfaceGroup.createSurface(url === undefined ? {} : { url })
    } else if (action === 'select') {
      const index = expectTabIndex(input.arguments.index, true)
      if (index === undefined) {
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
      }
      input.markDispatched()
      await this.surfaceGroup.selectSurface({ index })
    } else if (action === 'close') {
      const index = expectTabIndex(input.arguments.index, false)
      input.markDispatched()
      await this.surfaceGroup.closeSurfaceByIndex(index)
    } else {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
    }
    await this.disposeConnection(false, false)
    return tabsToolResult(this.surfaceGroup.listSurfaces())
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
      const owner = this.artifactOwner(input.options)
      const session = input.connection.outputSession
      if (!session) throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
      reservation = await session.reserveFile({
        owner,
        kind: 'trace',
        mimeType: 'application/zip',
        suggestedFileName: 'browser-trace.zip'
      })
      input.markDispatched()
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
    input.markDispatched()
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

  private artifactOwner(input: HostAdapterInput['options']): BrowserArtifactOwner {
    const authorization = input.authorizationContext
    const identity = this.getActiveSurfaceIdentity?.()
    if (!authorization || !identity) {
      throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    }
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
  }): Promise<ManagedUpstreamArtifactPlan | undefined> {
    const spec = artifactSpec(input.name, input.modelArguments)
    if (!spec) return undefined
    await this.getManagedBrowserContext()
    const session = input.connection.outputSession
    if (!session) throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
    const reservation = await session.reserveFile({
      owner: this.artifactOwner(input.options),
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
        await route.fulfill({ status: definition.status, body: '' })
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
    if (this.connection) return this.connection
    if (this.connecting) return this.connecting

    const epoch = this.connectionEpoch
    const connecting = this.createConnection()
    this.connecting = connecting
    try {
      const connection = await connecting
      if (this.closed || this.connectionEpoch !== epoch) {
        await this.closeConnection(connection)
        throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
      }
      this.connection = connection
      return connection
    } finally {
      if (this.connecting === connecting) this.connecting = undefined
    }
  }

  private async createConnection(): Promise<ActiveConnection> {
    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair()
    const outputSession = await this.artifactBroker?.openSession()
    const outputDirectory = outputSession?.outputDirectory ?? (await createSecureOutputDirectory())
    if (this.closed) {
      await (outputSession?.close() ?? removeOutputDirectory(outputDirectory))
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    if (!outputSession) this.outputDirectories.add(outputDirectory)
    let server: ManagedMcpServer | undefined
    let client: ManagedMcpClient | undefined
    try {
      server = await this.createOfficialConnection(
        {
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
        () => this.getManagedBrowserContext()
      )
      client = this.createClient()
      await Promise.all([server.connect(serverTransport), client.connect(clientTransport)])
      return {
        client,
        server,
        clientTransport,
        serverTransport,
        outputDirectory,
        ...(outputSession ? { outputSession } : {})
      }
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
      await (outputSession?.close() ?? removeOutputDirectory(outputDirectory))
      if (!outputSession) this.outputDirectories.delete(outputDirectory)
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
    requestedTimeoutMs?: number
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
      const result = await raceWithAbort(operation(controller.signal), controller.signal)
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
    cancelActiveCalls = true
  ): Promise<void> {
    this.connectionEpoch += 1
    await this.clearFrameEditorCandidates()
    if (cancelActiveCalls) {
      for (const controller of this.activeCalls) controller.abort('host_close')
    }
    const connection = this.connection
    this.connection = undefined
    const connecting = this.connecting
    this.connecting = undefined
    if (connecting) {
      // `ensureConnected` remains the single owner of a connection under construction. Epoch
      // invalidation makes that continuation close its late value exactly once; this bounded wait
      // merely gives graceful settlement a chance without double-closing or blocking shutdown.
      await settleWithin(connecting, CONNECTION_CLOSE_SETTLE_MS)
    }
    if (connection) await this.closeConnection(connection)
    const orphanedOutputDirectories = [...this.outputDirectories]
    this.outputDirectories.clear()
    await Promise.allSettled(orphanedOutputDirectories.map(removeOutputDirectory))
    if (detachAutomation) await this.detachAutomation().catch(() => undefined)
  }

  private async closeConnection(connection: ActiveConnection): Promise<void> {
    await closeConnection(connection)
    this.outputDirectories.delete(connection.outputDirectory)
  }
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

function artifactToolResult(
  artifactOrArtifacts: BrowserArtifactReference | readonly BrowserArtifactReference[]
): ManagedPlaywrightCallResult {
  const artifacts = Array.isArray(artifactOrArtifacts)
    ? [...artifactOrArtifacts]
    : [artifactOrArtifacts]
  const primary = artifacts[0]
  if (!primary) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  return {
    content: [
      {
        type: 'text',
        text:
          artifacts.length === 1
            ? `Created managed ${primary.kind} Artifact “${primary.displayName}” (${primary.sizeBytes} bytes).`
            : `Created ${artifacts.length} managed browser Artifacts.`
      }
    ],
    structuredContent: { status: 'completed', artifacts },
    isError: false
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

function tabsToolResult(
  surfaces: readonly ManagedPlaywrightSurfaceView[]
): ManagedPlaywrightCallResult {
  const tabs = surfaces.map((surface) => ({
    index: surface.index,
    title: surface.title,
    url: surface.url,
    current: surface.isActive
  }))
  return {
    content: [
      {
        type: 'text',
        text:
          tabs.length === 0
            ? 'No managed browser tabs are open.'
            : tabs
                .map(
                  (tab) =>
                    `${tab.current ? '- (current)' : '-'} ${tab.index}: ${tab.title} (${tab.url})`
                )
                .join('\n')
      }
    ],
    structuredContent: { tabs },
    isError: false
  }
}

function requiredRunId(authorization: BrowserRiskAuthorizationContext | undefined): string {
  const runId = authorization?.runId
  if (!runId || runId.length > 512) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return runId
}

function expectTabIndex(value: unknown, required: boolean): number | undefined {
  if (value === undefined && !required) return undefined
  if (!Number.isSafeInteger(value) || Number(value) < 0 || Number(value) > 15) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return Number(value)
}

function expectPositiveDimension(value: unknown): number {
  if (!Number.isSafeInteger(value) || Number(value) < 100 || Number(value) > 8_192) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return Number(value)
}

function safeRoutePattern(value: string): string {
  let sanitized = ''
  for (const character of value) {
    const code = character.charCodeAt(0)
    sanitized += code <= 0x1f || code === 0x7f ? ' ' : character
  }
  sanitized = sanitized.trim()
  return sanitized.length <= 256 ? sanitized : `${sanitized.slice(0, 253)}...`
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
            suggestedFileName: optionalSuggestion('browser-console.log')
          }
    case 'browser_network_requests':
      return suggested === undefined
        ? undefined
        : {
            kind: 'network',
            mimeType: 'text/plain',
            suggestedFileName: optionalSuggestion('browser-network.log')
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
  delete copy.approval_origin
  return copy
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

function parseBoundedToolResult(
  value: unknown,
  discardStructuredContent = false
): ManagedPlaywrightCallResult {
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
  const structuredContent = discardStructuredContent ? undefined : result.structuredContent
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

function isSafeDiagnosticTool(toolName: string): boolean {
  return toolName === 'browser_network_requests' || toolName === 'browser_console_messages'
}

function adaptReviewedToolResult(
  toolName: string,
  result: ManagedPlaywrightCallResult,
  modelArguments: Record<string, unknown>
): ManagedPlaywrightCallResult {
  const frameFailure = safeFrameFailureResult(toolName, result)
  if (frameFailure) return frameFailure
  if (toolName === 'browser_console_messages') {
    return safeConsoleSummaryResult(result, modelArguments.level)
  }
  if (toolName === 'browser_network_requests') {
    return safeNetworkSummaryResult(result)
  }
  if (!isSafeDiagnosticTool(toolName)) {
    return result
  }
  throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
}

const SAFE_NETWORK_METHODS = new Set(['DELETE', 'GET', 'HEAD', 'OPTIONS', 'PATCH', 'POST', 'PUT'])
const MAX_SAFE_NETWORK_ENTRIES = 512
const MAX_SAFE_NETWORK_INDEX = 1_000_000
const MAX_SAFE_NETWORK_URL_BYTES = 2_048

interface SafeNetworkEntry {
  readonly index: number
  readonly method: string
  readonly status: number | 'failed'
  readonly url: string
}

/**
 * Rebuilds the network ledger from a narrow grammar owned by the Host.
 *
 * A response status phrase, request failure text, and every unrecognized line are controlled by
 * the page or remote server, so none of those bytes are copied. The fixed official 0.0.79 format
 * is used only to recover an index, an allowlisted method, a safe HTTP(S) origin/path, and either a
 * numeric status or the literal FAILED state. Catalog drift fails closed to a count-only summary.
 */
function safeNetworkSummaryResult(
  result: ManagedPlaywrightCallResult
): ManagedPlaywrightCallResult {
  const textBlocks: string[] = []
  for (const block of result.content) {
    if (block.type !== 'text') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    textBlocks.push(block.text)
  }

  if (result.isError) {
    return {
      content: [
        {
          type: 'text',
          text: 'Managed browser network summary failed; upstream error text was omitted.'
        }
      ],
      structuredContent: {
        schemaVersion: 1,
        status: 'failed',
        summary: { count: 0, requests: [] },
        contentOmitted: true
      },
      isError: true
    }
  }

  const entries = parseOfficialNetworkEntries(textBlocks.join('\n'))
  const lines = entries.map(
    (entry) =>
      `${entry.index}. [${entry.method}] ${entry.url} => [${
        entry.status === 'failed' ? 'FAILED' : entry.status
      }]`
  )
  const countLabel = `${entries.length} reviewed request${entries.length === 1 ? '' : 's'}`
  return {
    content: [
      {
        type: 'text',
        text: [
          `Managed browser network summary: ${countLabel}.`,
          ...lines,
          'Status text, failure text, query strings, fragments, credentials, and unrecognized content were omitted.'
        ].join('\n')
      }
    ],
    structuredContent: {
      schemaVersion: 1,
      status: 'completed',
      summary: { count: entries.length, requests: entries },
      contentOmitted: true
    },
    isError: false
  }
}

function parseOfficialNetworkEntries(text: string): SafeNetworkEntry[] {
  const resultPrefix = /^### Result(?:\r?\n|$)/u.exec(text)
  if (!resultPrefix || resultPrefix.index !== 0) return []
  const resultBody = text.slice(resultPrefix[0].length).split(/\r?\n### /u, 1)[0]
  const entries: SafeNetworkEntry[] = []
  let previousIndex = 0
  for (const line of resultBody.split(/\r?\n/u)) {
    if (entries.length >= MAX_SAFE_NETWORK_ENTRIES) break
    const match =
      /^([1-9][0-9]{0,15})\. \[([A-Z]+)\] (\S+) => \[(FAILED|[0-9]{3})\](?: .*)?$/u.exec(line)
    if (!match || !SAFE_NETWORK_METHODS.has(match[2])) continue
    const index = Number(match[1])
    if (!Number.isSafeInteger(index) || index <= previousIndex || index > MAX_SAFE_NETWORK_INDEX) {
      continue
    }
    const url = safeNetworkUrl(match[3])
    if (!url) continue
    const status = match[4] === 'FAILED' ? 'failed' : Number(match[4])
    if (status !== 'failed' && (!Number.isSafeInteger(status) || status < 100 || status > 599)) {
      continue
    }
    entries.push({ index, method: match[2], status, url })
    previousIndex = index
  }
  return entries
}

function safeNetworkUrl(value: string): string | undefined {
  try {
    const parsed = new URL(value)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return undefined
    const safe = `${parsed.origin}${parsed.pathname}`
    if (Buffer.byteLength(safe, 'utf8') > MAX_SAFE_NETWORK_URL_BYTES) return undefined
    return safe
  } catch {
    return undefined
  }
}

type SafeConsoleLevel = 'error' | 'warning'

interface SafeConsoleCounts {
  readonly errors: number
  readonly returned: number
  readonly total: number
  readonly warnings: number
}

/**
 * Converts page-controlled console output into a Host-owned, value-free DTO.
 *
 * Official Playwright puts its count ledger at the beginning of the Result section. We parse only
 * that fixed metadata prefix and discard every remaining byte, including arbitrary message text.
 * If a future upstream release changes the prefix, counts are omitted instead of treating any page
 * string as trusted metadata. The Catalog lock will separately stop an unreviewed package update.
 */
function safeConsoleSummaryResult(
  result: ManagedPlaywrightCallResult,
  levelValue: unknown
): ManagedPlaywrightCallResult {
  const level = safeConsoleLevel(levelValue)
  const textBlocks: string[] = []
  for (const block of result.content) {
    if (block.type !== 'text') {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
    textBlocks.push(block.text)
  }

  if (result.isError) {
    return {
      content: [
        {
          type: 'text',
          text: `Managed browser console summary failed for level "${level}"; message text was omitted.`
        }
      ],
      structuredContent: {
        schemaVersion: 1,
        status: 'failed',
        summary: { level },
        contentOmitted: true
      },
      isError: true
    }
  }

  const counts = parseOfficialConsoleCounts(textBlocks.join('\n'), level)
  return {
    content: [
      {
        type: 'text',
        text: counts
          ? `Managed browser console summary: ${counts.total} total, ${counts.errors} errors, ${counts.warnings} warnings, ${counts.returned} returned for level "${level}". Message text was omitted.`
          : `Managed browser console summary completed for level "${level}". Message text was omitted.`
      }
    ],
    structuredContent: {
      schemaVersion: 1,
      status: 'completed',
      summary: { level, ...(counts ? { counts } : {}) },
      contentOmitted: true
    },
    isError: false
  }
}

function safeConsoleLevel(value: unknown): SafeConsoleLevel {
  if (value === 'error' || value === 'warning') return value
  throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
}

function parseOfficialConsoleCounts(
  text: string,
  requestedLevel: SafeConsoleLevel
): SafeConsoleCounts | undefined {
  const match =
    /^### Result\r?\nTotal messages: ([0-9]{1,16}) \(Errors: ([0-9]{1,16}), Warnings: ([0-9]{1,16})\)(?:\r?\nReturning ([0-9]{1,16}) messages for level "(error|warning|info|debug)")?(?:\r?\n|$)/u.exec(
      text
    )
  if (!match) return undefined
  const total = parseSafeConsoleCount(match[1])
  const errors = parseSafeConsoleCount(match[2])
  const warnings = parseSafeConsoleCount(match[3])
  const returned = match[4] === undefined ? total : parseSafeConsoleCount(match[4])
  const returnedLevel = match[5]
  if (
    total === undefined ||
    errors === undefined ||
    warnings === undefined ||
    returned === undefined ||
    errors + warnings > total ||
    returned > total ||
    (returnedLevel !== undefined && returnedLevel !== requestedLevel)
  ) {
    return undefined
  }
  return { errors, returned, total, warnings }
}

function parseSafeConsoleCount(value: string): number | undefined {
  const parsed = Number(value)
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : undefined
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
  await (connection.outputSession?.close() ?? removeOutputDirectory(connection.outputDirectory))
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

function boundedTimeout(value: number | undefined): number {
  if (value === undefined) return DEFAULT_TOOL_TIMEOUT_MS
  if (!Number.isSafeInteger(value) || value <= 0) return DEFAULT_TOOL_TIMEOUT_MS
  return Math.min(value, MAX_TOOL_TIMEOUT_MS)
}

/**
 * The proposal-time binding currently freezes the active top-level document, not an arbitrary
 * descendant Frame. Sensitive element operations therefore accept only a top-level snapshot ref;
 * selectors, `f<frame>e<element>` refs, frameLocator expressions and Host editor tokens are
 * intentionally rejected before connection/file consumption/upstream dispatch. File chooser
 * ownership is not exposed by the official Tool contract, so approved uploads stay unavailable
 * until Main can bind the exact chooser Frame instead of assuming the top-level origin.
 */
function assertSensitiveTopFrameScope(
  toolName: string,
  modelArguments: Readonly<Record<string, unknown>>
): void {
  if (toolName === 'browser_network_request') {
    throw new ManagedPlaywrightMcpHostError(
      'mcp.builtin_playwright.sensitive_request_identity_unavailable',
      'definitely_not_dispatched'
    )
  }
  if (toolName === 'browser_file_upload') {
    throw new ManagedPlaywrightMcpHostError(
      'mcp.builtin_playwright.sensitive_target_scope_unsupported',
      'definitely_not_dispatched'
    )
  }
  if (toolName !== 'browser_evaluate' && toolName !== 'browser_drop') return
  if (
    toolName === 'browser_drop' &&
    (!Array.isArray(modelArguments.paths) || modelArguments.paths.length === 0)
  ) {
    return
  }
  const target = modelArguments.target
  if (toolName === 'browser_evaluate' && target === undefined) return
  if (typeof target === 'string' && /^e\d+$/.test(target)) return
  throw new ManagedPlaywrightMcpHostError(
    'mcp.builtin_playwright.sensitive_target_scope_unsupported',
    'definitely_not_dispatched'
  )
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
