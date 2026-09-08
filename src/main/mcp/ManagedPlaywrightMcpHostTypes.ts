import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import type { BrowserContext, ElementHandle, Frame, Route } from 'playwright'
import type {
  BrowserAgentDownloadSnapshot,
  BrowserSurfacePresentation,
  BrowserSurfacePublicCrashError,
  BrowserSurfacePublicLoadError
} from '@mycopilot/protocol'
import { createConnection } from '@playwright/mcp'
import type {
  BrowserNetworkOperationLease,
  BrowserTargetCreationAuthority
} from '../browser/BrowserNetworkGuard'
import type {
  BrowserArtifactBroker,
  BrowserArtifactOutputSession,
  BrowserArtifactOwner,
  BrowserArtifactReservation
} from '../browser/BrowserArtifactBroker'
import type { BrowserFileBroker, BrowserFileReadLease } from '../browser/BrowserFileBroker'
import type { BrowserFrameEditorKind } from '../browser/BrowserFrameEditorProbe'
import type { BrowserRiskAuthorizationContext } from '../browser/BrowserRiskCoordinator'
import type {
  BrowserRunSurfaceOwner,
  BrowserToolSurfaceLeaseOptions
} from '../browser/BrowserSurfaceTypes'
import type { ManagedPlaywrightSensitiveGrantLease } from './managedPlaywrightSensitivePolicy'
import type {
  ManagedPlaywrightSensitiveTargetBindingLease,
  ManagedPlaywrightSensitiveTargetBindingStore,
  ManagedPlaywrightSensitiveTargetIdentity
} from './ManagedPlaywrightSensitiveTargetBindingBroker'
import { MANAGED_PLAYWRIGHT_PACKAGE_VERSION } from './managedPlaywrightManifest'

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
    onApprovalWaitChange?: (waiting: boolean) => void
  }) => Promise<BrowserNetworkOperationLease>
  beginTargetCreationOperation?: (input: {
    authorizationContext: BrowserRiskAuthorizationContext
    parentRequestId: string
    signal: AbortSignal
    onApprovalWaitChange?: (waiting: boolean) => void
  }) => Promise<BrowserNetworkOperationLease>
  getBrowserContext: () => Promise<BrowserContext>
  getAgentDownloadSnapshot?: (input: {
    runId: string
    activationId: string
    conversationId?: string
  }) => BrowserAgentDownloadSnapshot
  sensitiveTargetBindings?: ManagedPlaywrightSensitiveTargetBindingStore
  artifactBroker?: BrowserArtifactBroker
  fileBroker?: BrowserFileBroker
  finalizeBrowserRun?: (runId: string) => Promise<void>
  releaseBrowserCapability?: (activationId: string) => Promise<void>
  releaseBrowserToolCall?: (input: { runId: string; toolCallId: string }) => Promise<void>
  surfaceGroup: ManagedPlaywrightSurfaceGroupAdapter
  closeSurface: () => Promise<void>
  detachAutomation: () => Promise<void>
  toolTimeoutMs?: number
  /** Maximum wait for the shared browser dispatch slot; separate from Tool execution time. */
  queueTimeoutMs?: number
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  createClient?: () => ManagedMcpClient
}

export interface ManagedPlaywrightSurfaceView {
  crashError?: BrowserSurfacePublicCrashError | null
  surfaceId: string
  index: number
  title: string
  url: string
  isActive: boolean
  loadError?: BrowserSurfacePublicLoadError | null
  presentation?: BrowserSurfacePresentation
  generation: number
}

export interface ManagedPlaywrightSurfaceGroupAdapter {
  /** Main-only identity; never include this projection in Renderer surface ViewModels. */
  getSensitiveTargetIdentity(
    owner?: BrowserRunSurfaceOwner
  ): ManagedPlaywrightSensitiveTargetIdentity | null
  prepareRunTarget?(owner: BrowserRunSurfaceOwner): void
  releaseRunTarget?(runId: string): void
  clearRunTargets?(): void
  ensureActiveSurface(): Promise<ManagedPlaywrightSurfaceView>
  listSurfaces(): readonly ManagedPlaywrightSurfaceView[]
  createSurface(input?: { url?: string }): Promise<ManagedPlaywrightSurfaceView>
  selectSurface(input: { index: number }): Promise<ManagedPlaywrightSurfaceView>
  closeSurfaceByIndex(index?: number): Promise<void>
  /** Freezes the run's bound surface (or its initial trusted UI selection) for one dispatch. */
  beginToolSurfaceLease(
    options?: BrowserToolSurfaceLeaseOptions
  ): Promise<ManagedPlaywrightToolSurfaceLease>
  /** Locks the selected existing surface without creating or revealing a tab. */
  beginExistingToolSurfaceLease(
    options?: BrowserToolSurfaceLeaseOptions
  ): Promise<ManagedPlaywrightToolSurfaceLease | null>
  /** Locks one exact model-visible tab index without selecting or revealing it. */
  beginToolSurfaceLeaseByIndex(
    index: number,
    options?: BrowserToolSurfaceLeaseOptions
  ): Promise<ManagedPlaywrightToolSurfaceLease>
  /** Narrows Browser-level target creation to the reviewed Tool's expected UX. */
  beginTargetCreationIntent(
    intent: 'background' | 'interactive',
    authority?: BrowserTargetCreationAuthority,
    owner?: BrowserRunSurfaceOwner
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

export interface ManagedMcpServer {
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

export interface UpstreamToolDescriptor {
  name: string
  description?: string
  inputSchema: Record<string, unknown>
  annotations?: Record<string, unknown>
}

export interface ActiveConnection {
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

export interface ManagedConnectionOutputLifetime {
  retainDirectory(directory: string): () => Promise<void>
  retireConnection(): Promise<void>
}

export interface PreparedSensitiveFileLease extends BrowserFileReadLease {
  markDispatched?(): void
}

export interface ManagedRouteDefinition {
  addHeaders?: Readonly<Record<string, string>>
  body?: string
  contentType?: string
  pattern: string
  removeHeaders?: readonly string[]
  runId: string
  status?: number
}

export interface AppliedContextNetworkState {
  handleClose: () => void
  routes: Map<ManagedRouteDefinition, (route: Route) => Promise<void>>
  offline: boolean
}

export interface ManagedTracingState {
  context: BrowserContext
  owner: BrowserArtifactOwner
  reservation: BrowserArtifactReservation
}

export interface ManagedUpstreamArtifactPlan {
  hostWritesDiagnosticText: boolean
  reservation: BrowserArtifactReservation
  serverArguments: Record<string, unknown>
}

export interface HostAdapterInput {
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

export interface PreparedSensitiveGrant {
  readonly lease: ManagedPlaywrightSensitiveGrantLease
  readonly targetBinding: ManagedPlaywrightSensitiveTargetBindingLease
  readonly target?: ManagedPlaywrightSensitiveTargetIdentity
}

export interface RegisteredFrameEditorCandidate {
  readonly activationId?: string
  readonly element: ElementHandle<HTMLElement>
  readonly expiresAtMs: number
  readonly frame: Frame
  readonly generation: number
  readonly kind: BrowserFrameEditorKind
  readonly runId?: string
  readonly surfaceId: string
}
