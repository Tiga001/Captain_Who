import { chmod, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'
import { BrowserArtifactBrokerError } from '../browser/BrowserArtifactBroker'
import { BrowserDownloadBrokerError } from '../browser/BrowserDownloadBroker'
import { BrowserFileBrokerError } from '../browser/BrowserFileBroker'
import { matchesBoundedJsonSchema } from './boundedJsonSchema'
import { ManagedPlaywrightSensitiveGrantError } from './managedPlaywrightSensitivePolicy'
import type { ManagedPlaywrightToolManifestEntry } from './managedPlaywrightManifest'
import type {
  ActiveConnection,
  ManagedConnectionOutputLifetime,
  ManagedPlaywrightCallResult,
  ManagedPlaywrightContentBlock
} from './ManagedPlaywrightMcpHostTypes'
import { ManagedPlaywrightMcpHostError } from './ManagedPlaywrightMcpHostError'

const DEFAULT_TOOL_TIMEOUT_MS = 60_000
const MAX_TOOL_TIMEOUT_MS = 300_000
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

export async function settleWithin<T>(
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

export function expectArgumentRecord(value: unknown): Record<string, unknown> {
  const record = expectRecordWithoutThrow(value)
  if (!record) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
  }
  return record
}

export function stripHostArguments(record: Record<string, unknown>): Record<string, unknown> {
  const copy = { ...record }
  delete copy.call_reason
  return copy
}

export type ToolSurfaceLeaseMode = 'creating' | 'existing' | 'optional_existing' | 'none'

export function toolSurfaceLeaseMode(
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

export function shouldSynchronizeOfficialSurface(
  name: string,
  modelArguments: Readonly<Record<string, unknown>>
): boolean {
  if (name === 'browser_close') return false
  if (name === 'browser_tabs') return modelArguments.action === 'list'
  return toolSurfaceLeaseMode(name, modelArguments) !== 'none'
}

export function targetCreationIntentForTool(
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

export function toolUsesManagedPageNetworkOperation(
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

export function validateReviewedArguments(
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

export function parseBoundedToolResult(value: unknown): ManagedPlaywrightCallResult {
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

export function adaptReviewedToolResult(
  toolName: string,
  result: ManagedPlaywrightCallResult
): ManagedPlaywrightCallResult {
  const frameFailure = safeFrameFailureResult(toolName, result)
  if (frameFailure) return frameFailure
  return result
}

export type ManagedFrameFailureCode =
  | 'frame_not_found'
  | 'stale_frame_ref'
  | 'frame_not_editable'
  | 'frame_input_delivery_failed'
  | 'frame_detached'
  | 'target_closed'

export class ManagedFrameEditorTargetError extends Error {
  readonly name = 'ManagedFrameEditorTargetError'

  constructor(readonly code: ManagedFrameFailureCode) {
    super(code)
  }
}

export function managedFrameFailureResult(
  code: ManagedFrameFailureCode
): ManagedPlaywrightCallResult {
  return {
    content: [{ type: 'text', text: `Managed browser frame operation failed: ${code}.` }],
    structuredContent: { status: 'failed', errorCode: code, contentOmitted: true },
    isError: true
  }
}

export function safeFrameFailureResult(
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

export function classifyFrameFailure(diagnostic: string): ManagedFrameFailureCode | undefined {
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

export const TERMINAL_MANAGED_CONNECTION_PATTERN =
  /(?:target page, context or browser|page|browser|connection|session|transport) (?:has been |was |is )?closed|target_closed|transportclosed|protocol error[^\n]*session closed/iu

export function isTerminalManagedConnectionResult(result: ManagedPlaywrightCallResult): boolean {
  if (!result.isError) return false
  if (result.structuredContent?.errorCode === 'target_closed') return true
  const diagnostic = result.content
    .flatMap((block) => (block.type === 'text' ? [block.text] : []))
    .join('\n')
  return TERMINAL_MANAGED_CONNECTION_PATTERN.test(diagnostic)
}

export const FRAME_INTERACTION_TOOLS = new Set([
  'browser_click',
  'browser_fill_form',
  'browser_press_key',
  'browser_type'
])

export function cloneBoundedStructuredContent(value: object): Record<string, unknown> {
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

export function encodedMediaData(block: ManagedPlaywrightContentBlock): string | undefined {
  if (block.type === 'image' || block.type === 'audio') return block.data
  if (block.type === 'embedded_resource' && block.resource.contentType === 'blob') {
    return block.resource.data
  }
  return undefined
}

export class ContentBlockBudget {
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

export function jsonEncodedStringBytes(value: string): number {
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

export function parseContentBlock(
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

export async function closeConnection(connection: ActiveConnection): Promise<void> {
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

export function observeManagedProtocolClose(
  peer: { onclose?: () => void },
  observer: () => void
): void {
  const previous = peer.onclose
  peer.onclose = () => {
    try {
      previous?.()
    } finally {
      observer()
    }
  }
}

export function createManagedConnectionOutputLifetime(
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

export async function createSecureOutputDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), 'mycopilot-playwright-mcp-'))
  // `mkdtemp` is already private on POSIX. Enforce the intended boundary explicitly so a
  // permissive process umask or future platform implementation cannot widen access.
  await chmod(directory, 0o700)
  return directory
}

export async function removeOutputDirectory(directory: string): Promise<void> {
  await rm(directory, { force: true, recursive: true }).catch(() => undefined)
}

export function onceAsync(operation: () => Promise<void>): () => Promise<void> {
  let pending: Promise<void> | undefined
  return () => (pending ??= Promise.resolve().then(operation))
}

/** Tool execution timeout which excludes time spent waiting for an explicit human decision. */
export class PausableCallTimeout {
  private disposed = false
  private remainingMs: number
  private resumedAt = Date.now()
  private timer?: ReturnType<typeof setTimeout>
  private waitingDepth = 0

  constructor(
    private readonly controller: AbortController,
    timeoutMs: number
  ) {
    this.remainingMs = timeoutMs
    this.schedule()
  }

  setWaiting(waiting: boolean): void {
    if (this.disposed || this.controller.signal.aborted) return
    if (waiting) {
      this.waitingDepth += 1
      if (this.waitingDepth !== 1) return
      this.remainingMs = Math.max(0, this.remainingMs - (Date.now() - this.resumedAt))
      if (this.timer) clearTimeout(this.timer)
      this.timer = undefined
      return
    }
    if (this.waitingDepth === 0) return
    this.waitingDepth -= 1
    if (this.waitingDepth === 0) this.schedule()
  }

  dispose(): void {
    this.disposed = true
    if (this.timer) clearTimeout(this.timer)
    this.timer = undefined
  }

  private schedule(): void {
    if (this.disposed || this.controller.signal.aborted || this.waitingDepth > 0) return
    this.resumedAt = Date.now()
    this.timer = setTimeout(() => this.controller.abort('timeout'), Math.max(1, this.remainingMs))
  }
}

export function boundedTimeout(value: number | undefined): number {
  if (value === undefined) return DEFAULT_TOOL_TIMEOUT_MS
  if (!Number.isSafeInteger(value) || value <= 0) return DEFAULT_TOOL_TIMEOUT_MS
  return Math.min(value, MAX_TOOL_TIMEOUT_MS)
}

export function mapSafeHostError(error: unknown): ManagedPlaywrightMcpHostError {
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
      case 'browser.download.destination_unavailable':
      case 'browser.download.registration_failed':
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

export function cancellationError(reason: unknown): ManagedPlaywrightMcpHostError {
  return new ManagedPlaywrightMcpHostError(
    reason === 'timeout' ? 'mcp.builtin_playwright.timeout' : 'mcp.builtin_playwright.cancelled'
  )
}

export function expectRecord(value: unknown): Record<string, unknown> {
  const record = expectRecordWithoutThrow(value)
  if (!record) {
    throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.protocol_error')
  }
  return record
}

export function expectRecordWithoutThrow(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}
