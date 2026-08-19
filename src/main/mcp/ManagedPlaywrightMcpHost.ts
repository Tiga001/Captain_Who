import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js'
import type { BrowserContext } from 'playwright'
import { createConnection } from '@playwright/mcp'
import { chmod, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'

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
  | 'mcp.builtin_playwright.catalog_drift'
  | 'mcp.builtin_playwright.output_too_large'
  | 'mcp.builtin_playwright.protocol_error'
  | 'browser.surface_unavailable'
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
  annotations: {
    readOnlyHint: boolean
    destructiveHint: boolean
  }
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
  closeSurface: () => Promise<void>
  detachAutomation: () => Promise<void>
  toolTimeoutMs?: number
  createOfficialConnection?: ManagedPlaywrightConnectionFactory
  createClient?: () => ManagedMcpClient
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
}

interface ActiveConnection {
  client: ManagedMcpClient
  server: ManagedMcpServer
  clientTransport: InMemoryTransport
  serverTransport: InMemoryTransport
  outputDirectory: string
}

/**
 * Main-process, in-memory host for the exact official Playwright MCP package.
 *
 * Discovery never resolves a BrowserContext. The official server receives the context getter and
 * invokes it lazily on the first real browser operation. Every model-visible schema comes from the
 * reviewed Host manifest, and calls outside that manifest are rejected before reaching upstream.
 */
export class ManagedPlaywrightMcpHost {
  private readonly beginNetworkOperation?: ManagedPlaywrightMcpHostOptions['beginNetworkOperation']
  private readonly closeSurface: () => Promise<void>
  private readonly createClient: () => ManagedMcpClient
  private readonly createOfficialConnection: ManagedPlaywrightConnectionFactory
  private readonly detachAutomation: () => Promise<void>
  private readonly getBrowserContext: () => Promise<BrowserContext>
  private readonly toolTimeoutMs: number

  private readonly activeCalls = new Set<AbortController>()
  private readonly outputDirectories = new Set<string>()
  private connection?: ActiveConnection
  private connecting?: Promise<ActiveConnection>
  private connectionEpoch = 0
  private closed = false

  constructor(options: ManagedPlaywrightMcpHostOptions) {
    this.beginNetworkOperation = options.beginNetworkOperation
    this.getBrowserContext = options.getBrowserContext
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
      await this.runBounded(() => this.ensureConnected(), signal)
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

  async listTools(signal?: AbortSignal): Promise<readonly ManagedPlaywrightToolDescriptor[]> {
    const upstream = await this.runBounded(async (operationSignal) => {
      const connection = await this.ensureConnected()
      return connection.client.listTools(undefined, {
        signal: operationSignal,
        timeout: this.toolTimeoutMs
      })
    }, signal)
    validateUpstreamCatalog(upstream.tools)
    return MANAGED_PLAYWRIGHT_MANIFEST.tools.map((tool) => ({
      name: tool.rawName,
      description: tool.description,
      inputSchema: structuredClone(tool.inputSchema),
      annotations: {
        readOnlyHint: tool.safety === 'read_only',
        destructiveHint: tool.safety === 'destructive'
      }
    }))
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
    const serverArguments = stripHostCallReason(modelArguments)

    if (name === 'browser_close') {
      await this.runBounded(async () => this.closeSurface(), options.signal, options.timeoutMs)
      await this.disposeConnection(false)
      return {
        content: [{ type: 'text', text: 'The managed browser page was closed.' }],
        isError: false
      }
    }

    let dispatchStarted = false
    let responseReceived = false
    try {
      return await this.runBounded(
        async (operationSignal) => {
          let riskLease: BrowserNetworkOperationLease | undefined
          try {
            if (this.beginNetworkOperation) {
              if (!options.authorizationContext || !options.parentRequestId) {
                throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
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

            const connection = await this.ensureConnected()
            let rawResult: unknown
            try {
              // Once the official MCP handler receives the call, page script, navigation, or form
              // submission may already have happened. Later network refusals are therefore never
              // represented as a safe pre-dispatch denial and must not be replayed automatically.
              riskLease?.markDispatched()
              dispatchStarted = true
              rawResult = await connection.client.callTool(
                { name, arguments: serverArguments },
                undefined,
                {
                  signal: operationSignal,
                  timeout: boundedTimeout(options.timeoutMs ?? this.toolTimeoutMs),
                  resetTimeoutOnProgress: false
                }
              )
              responseReceived = true
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
            try {
              await riskLease?.settle()
            } catch (error) {
              const pendingFailure = riskLease?.failure()
              if (pendingFailure) return riskFailureResult(pendingFailure)
              throw error
            }
            const failure = riskLease?.failure()
            if (failure) return riskFailureResult(failure)
            return parseBoundedToolResult(rawResult)
          } finally {
            riskLease?.finish()
          }
        },
        options.signal,
        options.timeoutMs
      )
    } catch (error) {
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

  /** Stops managed automation without closing the user's right-sidebar browser page. */
  async close(): Promise<void> {
    if (this.closed) return
    this.closed = true
    await this.disposeConnection(true)
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
    const outputDirectory = await createSecureOutputDirectory()
    if (this.closed) {
      await removeOutputDirectory(outputDirectory)
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    }
    this.outputDirectories.add(outputDirectory)
    let server: ManagedMcpServer | undefined
    let client: ManagedMcpClient | undefined
    try {
      server = await this.createOfficialConnection(
        {
          browser: { isolated: false },
          capabilities: ['core'],
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
        this.getBrowserContext
      )
      client = this.createClient()
      await Promise.all([server.connect(serverTransport), client.connect(clientTransport)])
      return { client, server, clientTransport, serverTransport, outputDirectory }
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
      await removeOutputDirectory(outputDirectory)
      this.outputDirectories.delete(outputDirectory)
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
    }
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

  private async disposeConnection(detachAutomation: boolean): Promise<void> {
    this.connectionEpoch += 1
    for (const controller of this.activeCalls) controller.abort('host_close')
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

function validateUpstreamCatalog(upstreamTools: readonly UpstreamToolDescriptor[]): void {
  const byName = new Map(upstreamTools.map((tool) => [tool.name, tool] as const))
  for (const reviewed of MANAGED_PLAYWRIGHT_MANIFEST.tools) {
    const upstream = byName.get(reviewed.rawName)
    const serverSchema = withoutHostCallReasonSchema(reviewed.inputSchema)
    if (!upstream || !reviewedSchemaIsCompatible(serverSchema, upstream.inputSchema)) {
      throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.catalog_drift')
    }
  }
}

function reviewedSchemaIsCompatible(reviewed: unknown, upstream: unknown, key?: string): boolean {
  if (key === 'description') return typeof reviewed === 'string'
  if (Array.isArray(reviewed)) {
    if (!Array.isArray(upstream)) return false
    if (key === 'enum') return reviewed.every((value) => upstream.includes(value))
    if (key === 'required') {
      return (
        reviewed.length === upstream.length && reviewed.every((value) => upstream.includes(value))
      )
    }
    return (
      reviewed.length === upstream.length &&
      reviewed.every((value, index) => reviewedSchemaIsCompatible(value, upstream[index]))
    )
  }
  if (reviewed !== null && typeof reviewed === 'object') {
    if (upstream === null || typeof upstream !== 'object' || Array.isArray(upstream)) return false
    const upstreamRecord = upstream as Record<string, unknown>
    return Object.entries(reviewed as Record<string, unknown>).every(([childKey, value]) =>
      reviewedSchemaIsCompatible(value, upstreamRecord[childKey], childKey)
    )
  }
  return Object.is(reviewed, upstream)
}

function expectArgumentRecord(value: unknown): Record<string, unknown> {
  const record = expectRecordWithoutThrow(value)
  if (!record) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return record
}

function stripHostCallReason(record: Record<string, unknown>): Record<string, unknown> {
  const copy = { ...record }
  delete copy.call_reason
  return copy
}

function withoutHostCallReasonSchema(
  schemaValue: Readonly<Record<string, unknown>>
): Record<string, unknown> {
  const schema = structuredClone(schemaValue) as Record<string, unknown>
  const properties = expectRecordWithoutThrow(schema.properties)
  if (properties) delete properties.call_reason
  if (Array.isArray(schema.required)) {
    const required = schema.required.filter((value) => value !== 'call_reason')
    if (required.length === 0) delete schema.required
    else schema.required = required
  }
  return schema
}

function validateReviewedArguments(
  tool: ManagedPlaywrightToolManifestEntry,
  value: Record<string, unknown>
): void {
  if (Buffer.byteLength(JSON.stringify(value), 'utf8') > MAX_ARGUMENT_BYTES) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  let nodes = 0
  if (!matchesSchema(value, tool.inputSchema, 0, () => ++nodes)) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
}

function matchesSchema(
  value: unknown,
  schemaValue: unknown,
  depth: number,
  countNode: () => number
): boolean {
  if (depth > MAX_ARGUMENT_DEPTH || countNode() > MAX_ARGUMENT_NODES) return false
  const schema = expectRecordWithoutThrow(schemaValue)
  if (!schema) return false
  if (schema.type === 'object') {
    const record = expectRecordWithoutThrow(value)
    const properties = expectRecordWithoutThrow(schema.properties) ?? {}
    if (!record) return false
    if (
      schema.additionalProperties === false &&
      Object.keys(record).some((key) => !(key in properties))
    ) {
      return false
    }
    if (
      Array.isArray(schema.required) &&
      schema.required.some((key) => typeof key !== 'string' || !(key in record))
    ) {
      return false
    }
    return Object.entries(record).every(([key, child]) =>
      key in properties ? matchesSchema(child, properties[key], depth + 1, countNode) : true
    )
  }
  if (schema.type === 'array') {
    return (
      Array.isArray(value) &&
      value.length <= 256 &&
      value.every((child) => matchesSchema(child, schema.items, depth + 1, countNode))
    )
  }
  if (schema.type === 'string') {
    if (typeof value !== 'string') return false
    if (typeof schema.minLength === 'number' && value.length < schema.minLength) return false
    if (typeof schema.maxLength === 'number' && value.length > schema.maxLength) return false
  }
  if (schema.type === 'number' && (typeof value !== 'number' || !Number.isFinite(value)))
    return false
  if (schema.type === 'integer' && (typeof value !== 'number' || !Number.isSafeInteger(value)))
    return false
  if (schema.type === 'boolean' && typeof value !== 'boolean') return false
  if (Array.isArray(schema.enum) && !schema.enum.includes(value)) return false
  return true
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
  await removeOutputDirectory(connection.outputDirectory)
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

function mapSafeHostError(error: unknown): ManagedPlaywrightMcpHostError {
  if (error instanceof ManagedPlaywrightMcpHostError) return error
  if (error !== null && typeof error === 'object' && 'code' in error) {
    const code = (error as { code?: unknown }).code
    if (code === 'browser.surface_unavailable' || code === 'browser.target_closed') {
      return new ManagedPlaywrightMcpHostError(code)
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
