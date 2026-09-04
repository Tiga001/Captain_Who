import type { BrowserContext, Locator, Page } from 'playwright'
import type {
  BrowserAgentDownloadSnapshot,
  BrowserAgentDownloadStatus,
  BrowserArtifactKind,
  BrowserArtifactReference,
  BrowserDownloadReference
} from '@mycopilot/protocol'
import { createRequire } from 'node:module'
import { extname, join } from 'node:path'
import type { BrowserNetworkOperationLease } from '../browser/BrowserNetworkGuard'
import {
  formatBrowserFrameEditorCandidates,
  probeBrowserFrameEditors,
  type BrowserFrameEditorCandidate
} from '../browser/BrowserFrameEditorProbe'
import type {
  BrowserRiskAuthorizationContext,
  BrowserRiskFailure
} from '../browser/BrowserRiskCoordinator'
import { MANAGED_PLAYWRIGHT_PACKAGE_VERSION } from './managedPlaywrightManifest'
import type {
  ManagedPlaywrightCallResult,
  ManagedPlaywrightSurfaceView,
  ManagedRouteDefinition
} from './ManagedPlaywrightMcpHostTypes'
import { ManagedPlaywrightMcpHostError } from './ManagedPlaywrightMcpHostError'
import { cancellationError, expectRecordWithoutThrow } from './managedPlaywrightProtocolSupport'

const PDF_UNAVAILABLE_SENTINEL = 'browser.pdf_unavailable'

export function safeOfficialCallFailureReason(error: unknown): string {
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

export type FixedPlaywrightTargetParser = (
  language: 'javascript',
  locator: string,
  testIdAttributeName?: string
) => string

let fixedPlaywrightTargetParser: FixedPlaywrightTargetParser | undefined

export function managedDropTargetLocator(page: Page, target: string): Locator {
  if (/^(f\d+)?e\d+$/u.test(target)) return page.locator(`aria-ref=${target}`)
  const selector = loadFixedPlaywrightTargetParser()('javascript', target, 'data-testid')
  if (!selector) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return page.locator(selector)
}

export function loadFixedPlaywrightTargetParser(): FixedPlaywrightTargetParser {
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

export async function raceWithAbort<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) throw cancellationError(signal.reason)
  return new Promise<T>((resolve, reject) => {
    const abort = (): void => reject(cancellationError(signal.reason))
    signal.addEventListener('abort', abort, { once: true })
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

export function riskFailureResult(failure: BrowserRiskFailure | null): ManagedPlaywrightCallResult {
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
      text =
        'This destination crosses a Captain Who Host isolation boundary and cannot be approved.'
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

export function textToolResult(text: string): ManagedPlaywrightCallResult {
  return { content: [{ type: 'text', text }], isError: false }
}

export function browserSurfaceFailureResult(
  surface: ManagedPlaywrightSurfaceView
): ManagedPlaywrightCallResult {
  if (surface.loadError) {
    return {
      content: [
        {
          type: 'text',
          text: 'The managed browser target is displaying a load error. Navigate or retry before interacting with the page.'
        }
      ],
      structuredContent: {
        status: 'load_error',
        generation: surface.generation,
        surfaceId: surface.surfaceId,
        loadError: surface.loadError
      },
      isError: true
    }
  }
  if (surface.crashError) {
    return {
      content: [
        {
          type: 'text',
          text: 'The managed browser target requires renderer recovery before page interaction.'
        }
      ],
      structuredContent: {
        status: 'renderer_failure',
        generation: surface.generation,
        surfaceId: surface.surfaceId,
        crashError: surface.crashError
      },
      isError: true
    }
  }
  throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
}

export function managedBrowserConfigResult(): ManagedPlaywrightCallResult {
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

export function artifactToolResult(
  artifactOrArtifacts: BrowserArtifactReference | readonly BrowserArtifactReference[],
  options: {
    downloads?: readonly BrowserDownloadReference[]
    hostImagePublishPath?: string
    readPathUnavailable?: 'too_large'
  } = {}
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
    structuredContent: {
      status: 'completed',
      artifacts,
      ...(options.downloads?.length ? { downloads: [...options.downloads] } : {})
    },
    isError: false,
    ...(options.hostImagePublishPath ? { hostImagePublishPath: options.hostImagePublishPath } : {})
  }
}

export function isPdfUnavailableResult(result: ManagedPlaywrightCallResult): boolean {
  return result.content.some(
    (block) => block.type === 'text' && block.text.includes(PDF_UNAVAILABLE_SENTINEL)
  )
}

export function pdfUnavailableToolResult(): ManagedPlaywrightCallResult {
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

export function withDownloadReferences(
  result: ManagedPlaywrightCallResult,
  downloads: readonly BrowserDownloadReference[]
): ManagedPlaywrightCallResult {
  if (downloads.length === 0) return result
  const summaries = downloads.map(
    (download) =>
      `Downloaded “${download.displayName}” (${download.mimeType}, ${download.sizeBytes} bytes). Durable reference: ${download.downloadId}`
  )
  return {
    ...result,
    content: [...result.content, ...summaries.map((text) => ({ type: 'text' as const, text }))],
    structuredContent: {
      ...(result.structuredContent ?? {}),
      downloads: [...downloads]
    }
  }
}

export function downloadTakeoverResult(): ManagedPlaywrightCallResult {
  return {
    content: [
      {
        type: 'text',
        text: 'The page action started a managed browser download. The original navigation was taken over by Electron; this is a successful download start, not a page-click failure.'
      }
    ],
    structuredContent: { status: 'download_started' },
    isError: false
  }
}

export function networkLeaseDownloadProgress(
  lease: BrowserNetworkOperationLease | undefined
): readonly BrowserAgentDownloadStatus[] {
  if (!lease || typeof lease.downloadProgress !== 'function') return []
  return lease.downloadProgress()
}

export function withAgentDownloadProgress(
  result: ManagedPlaywrightCallResult,
  snapshot: BrowserAgentDownloadSnapshot | undefined
): ManagedPlaywrightCallResult {
  if (!snapshot || snapshot.downloads.length === 0) return result
  const progress = snapshot.downloads.map((download) => structuredClone(download))
  const existing = Array.isArray(result.structuredContent?.downloads)
    ? (result.structuredContent.downloads as BrowserDownloadReference[])
    : []
  const completed = progress
    .map((download) => download.reference)
    .filter((reference): reference is BrowserDownloadReference => reference !== undefined)
  const downloads = [...existing]
  const ids = new Set(downloads.map((download) => download.downloadId))
  for (const reference of completed) {
    if (!ids.has(reference.downloadId)) {
      ids.add(reference.downloadId)
      downloads.push(reference)
    }
  }
  return {
    ...result,
    content: [
      ...result.content,
      ...progress.map((download) => ({
        type: 'text' as const,
        text: agentDownloadProgressText(download)
      }))
    ],
    structuredContent: {
      ...(result.structuredContent ?? {}),
      downloadProgress: progress,
      ...(downloads.length > 0 ? { downloads } : {})
    }
  }
}

export function agentDownloadProgressText(download: BrowserAgentDownloadStatus): string {
  const transferred = `${formatDownloadBytes(download.receivedBytes)} / ${formatDownloadBytes(download.totalBytes)}`
  switch (download.state) {
    case 'awaiting_destination':
      return `Download “${download.displayName}” is waiting for the user to choose a save location.`
    case 'progressing':
      return `Downloading “${download.displayName}”: ${transferred}${download.bytesPerSecond > 0 ? ` at ${formatDownloadBytes(download.bytesPerSecond)}/s` : ''}. Download ID: ${download.downloadId}`
    case 'paused':
      return `Download “${download.displayName}” is paused at ${transferred}. Download ID: ${download.downloadId}`
    case 'finalizing':
      return `Download “${download.displayName}” received all bytes and is being verified and registered. Download ID: ${download.downloadId}`
    case 'completed':
      return `Downloaded “${download.displayName}” (${formatDownloadBytes(download.receivedBytes)}). Durable reference: ${download.downloadId}`
    case 'cancelled':
      return `Download “${download.displayName}” was cancelled. Download ID: ${download.downloadId}`
    case 'interrupted':
    case 'failed':
      return `Download “${download.displayName}” failed with ${download.errorCode ?? 'browser.download.interrupted'}. Download ID: ${download.downloadId}`
  }
}

export function formatDownloadBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B'
  const units = ['B', 'KiB', 'MiB', 'GiB'] as const
  const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1)
  const amount = value / 1024 ** unit
  return `${unit === 0 ? Math.round(amount) : amount.toFixed(amount >= 100 ? 0 : 1)} ${units[unit]}`
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

export function requiredRunId(authorization: BrowserRiskAuthorizationContext | undefined): string {
  const runId = authorization?.runId
  if (!runId || runId.length > 512) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return runId
}

export function expectPositiveDimension(value: unknown): number {
  if (!Number.isSafeInteger(value) || Number(value) < 100 || Number(value) > 8_192) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return Number(value)
}

export function managedRouteDefinition(
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

export function managedRouteListLine(route: ManagedRouteDefinition, index: number): string {
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

export function artifactSpec(
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
