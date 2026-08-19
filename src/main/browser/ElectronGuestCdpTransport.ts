import { randomUUID } from 'node:crypto'
import type { ConnectOverCDPTransport } from 'playwright'
import type { Debugger, Event as ElectronEvent, WebContents } from 'electron'

type CdpParams = Record<string, unknown>

interface CdpRequest {
  id: number
  method: string
  params?: CdpParams
  sessionId?: string
}

interface CdpTargetInfo {
  attached: true
  browserContextId: string
  canAccessOpener: false
  targetId: string
  title: string
  type: 'page'
  url: string
}

interface ChildReadiness {
  readonly promise: Promise<boolean>
  settle(ready: boolean): void
}

interface ChildFocusProbe {
  readonly sessionId: string
  readonly frameId: string
}

const MAX_METHOD_LENGTH = 256
const MAX_SESSION_ID_LENGTH = 512
const MAX_CDP_STRING_LENGTH = 4_096
const MAX_INSERT_TEXT_BYTES = 64 * 1_024
const MAX_CDP_PAYLOAD_BYTES = 4 * 1_024 * 1_024
const MAX_CDP_PAYLOAD_DEPTH = 32
const MAX_CDP_PAYLOAD_NODES = 4_096
const MAX_CDP_CONTAINER_ITEMS = 4_096
const MAX_CDP_OBJECT_PROPERTIES = 512
const MAX_CDP_PROPERTY_NAME_BYTES = 256
const MAX_CDP_VALUE_STRING_BYTES = 1 * 1_024 * 1_024
const MAX_CDP_EVENT_QUEUE = 1_024
const MAX_CDP_EVENTS_PER_SECOND = 4_096
const MAX_CHILD_SESSIONS = 64
const FOCUS_PROBE_TIMEOUT_MS = 500

export class ElectronGuestCdpTransport implements ConnectOverCDPTransport {
  private readonly childSessionIds = new Set<string>()
  private readonly childReadiness = new Map<string, ChildReadiness>()
  private readonly childFrameIds = new Map<string, string>()
  private readonly childTargetIds = new Map<string, string>()
  private readonly debuggerClient: Debugger
  private readonly focusWorldName = `mycopilot-focus-${randomUUID()}`
  private readonly syntheticSessionId = `mycopilot-page-${randomUUID()}`
  private targetInfo: CdpTargetInfo
  private browserVersion: CdpParams = {
    jsVersion: '',
    product: 'Chrome/0.0.0.0',
    protocolVersion: '1.3',
    revision: '',
    userAgent: 'MyCopilot managed browser target'
  }
  private closeHandler?: (reason?: string) => void
  private closeNotificationDelivered = false
  private closeReason?: string
  private closed = false
  private eventCount = 0
  private eventQueue: object[] = []
  private eventWindowStartedAt = Date.now()
  private flushScheduled = false
  private emittedAttached = false
  private messageHandler?: (message: object) => void
  private ownsDebugger = false

  constructor(
    private readonly guest: WebContents,
    private readonly handleClosed: (transport: ElectronGuestCdpTransport) => void
  ) {
    this.debuggerClient = guest.debugger
    this.targetInfo = {
      attached: true,
      browserContextId: `mycopilot-context-${randomUUID()}`,
      canAccessOpener: false,
      targetId: `mycopilot-target-${randomUUID()}`,
      title: boundedString(guest.getTitle()),
      type: 'page',
      url: boundedString(guest.getURL())
    }
  }

  get onmessage(): ((message: object) => void) | undefined {
    return this.messageHandler
  }

  set onmessage(handler: ((message: object) => void) | undefined) {
    this.messageHandler = handler
  }

  get onclose(): ((reason?: string) => void) | undefined {
    return this.closeHandler
  }

  set onclose(handler: ((reason?: string) => void) | undefined) {
    this.closeHandler = handler
    this.deliverCloseNotification()
  }

  async attach(): Promise<void> {
    if (this.closed || this.guest.isDestroyed()) {
      throw new Error('Managed browser target is closed')
    }
    if (this.debuggerClient.isAttached()) {
      throw new Error('Managed browser target is already controlled')
    }

    this.debuggerClient.on('message', this.handleDebuggerMessage)
    this.debuggerClient.on('detach', this.handleDebuggerDetach)
    this.guest.once('destroyed', this.handleGuestDestroyed)

    try {
      this.debuggerClient.attach('1.3')
      this.ownsDebugger = true
      const [version, target] = (await Promise.all([
        this.debuggerClient.sendCommand('Browser.getVersion'),
        this.debuggerClient.sendCommand('Target.getTargetInfo')
      ])) as [unknown, unknown]
      this.browserVersion = normalizeBrowserVersion(version)
      this.targetInfo = normalizeTargetInfo(target, this.targetInfo)
    } catch {
      this.terminate('target_attach_failed')
      throw new Error('Unable to attach to managed browser target')
    }
  }

  open(): void {
    // The broker performs the asynchronous debugger attach before returning this transport.
  }

  send(message: object): void {
    if (this.closed) return
    const request = parseRequest(message)
    if (!request) {
      this.terminate('invalid_cdp_request')
      return
    }
    void this.dispatch(request)
  }

  close(): void {
    this.terminate('client_closed')
  }

  private readonly handleDebuggerMessage = (
    _event: ElectronEvent,
    method: string,
    params: unknown,
    sessionId: string
  ): void => {
    if (this.closed || typeof method !== 'string' || method.length > MAX_METHOD_LENGTH) return

    let safeParams: CdpParams
    try {
      safeParams = isRecord(params) ? cloneBoundedCdpRecord(params) : {}
    } catch {
      this.terminate('cdp_event_budget_exceeded')
      return
    }
    if (method === 'Target.attachedToTarget') {
      const attached = this.validateAttachedChild(safeParams, sessionId)
      if (!attached) return
      if (this.childSessionIds.size >= MAX_CHILD_SESSIONS) {
        this.terminate('cdp_child_session_limit_exceeded')
        return
      }
      this.childSessionIds.add(attached.sessionId)
      this.childTargetIds.set(attached.sessionId, attached.targetId)
      this.childReadiness.set(
        attached.sessionId,
        attached.waitingForDebugger ? createChildReadiness() : resolvedChildReadiness()
      )
    }

    const mappedSessionId = sessionId || this.syntheticSessionId
    if (mappedSessionId !== this.syntheticSessionId && !this.childSessionIds.has(mappedSessionId)) {
      return
    }

    if (
      mappedSessionId !== this.syntheticSessionId &&
      ((method === 'Page.lifecycleEvent' && safeParams.name === 'commit') ||
        method === 'Page.frameNavigated')
    ) {
      if (method === 'Page.frameNavigated') {
        const frame = safeParams.frame
        if (isRecord(frame) && typeof frame.id === 'string' && frame.id.length > 0) {
          this.childFrameIds.set(mappedSessionId, frame.id)
        }
      }
      this.childReadiness.get(mappedSessionId)?.settle(true)
    }

    const event = { method, params: safeParams, sessionId: mappedSessionId }
    this.enqueueEvent(event)

    if (method === 'Target.detachedFromTarget') {
      const childSessionId = safeParams.sessionId
      if (typeof childSessionId === 'string') {
        this.childSessionIds.delete(childSessionId)
        this.childFrameIds.delete(childSessionId)
        this.childTargetIds.delete(childSessionId)
        this.childReadiness.get(childSessionId)?.settle(false)
        this.childReadiness.delete(childSessionId)
      }
    }
  }

  private validateAttachedChild(
    params: CdpParams,
    parentSessionId: string
  ): { sessionId: string; targetId: string; waitingForDebugger: boolean } | null {
    const childSessionId = params.sessionId
    const targetInfo = params.targetInfo
    if (
      typeof childSessionId !== 'string' ||
      childSessionId.length === 0 ||
      childSessionId.length > MAX_SESSION_ID_LENGTH ||
      !isRecord(targetInfo) ||
      targetInfo.type !== 'iframe' ||
      targetInfo.attached !== true ||
      typeof targetInfo.targetId !== 'string' ||
      targetInfo.targetId.length === 0 ||
      targetInfo.targetId.length > MAX_SESSION_ID_LENGTH ||
      targetInfo.browserContextId !== this.targetInfo.browserContextId ||
      (params.waitingForDebugger !== true && params.waitingForDebugger !== false)
    ) {
      return null
    }
    const expectedParentTarget = parentSessionId
      ? this.childTargetIds.get(parentSessionId)
      : this.targetInfo.targetId
    if (!expectedParentTarget || targetInfo.parentFrameId !== expectedParentTarget) return null
    if (
      this.childSessionIds.has(childSessionId) ||
      [...this.childTargetIds.values()].includes(targetInfo.targetId)
    ) {
      return null
    }
    return {
      sessionId: childSessionId,
      targetId: targetInfo.targetId,
      waitingForDebugger: params.waitingForDebugger
    }
  }

  private readonly handleDebuggerDetach = (): void => {
    this.ownsDebugger = false
    this.terminate('target_detached')
  }

  private readonly handleGuestDestroyed = (): void => {
    this.ownsDebugger = false
    this.terminate('target_closed')
  }

  private async dispatch(request: CdpRequest): Promise<void> {
    try {
      const rawResult = await this.dispatchCommand(request)
      const result = cloneBoundedCdpPayload(rawResult === undefined ? {} : rawResult)
      this.respond(request, { result })
    } catch (error) {
      this.respond(request, {
        error: {
          code: -32_000,
          message: error instanceof CdpPolicyError ? error.message : 'Managed target command failed'
        }
      })
    }
  }

  private async dispatchCommand(request: CdpRequest): Promise<unknown> {
    if (this.closed || this.guest.isDestroyed()) throw new CdpPolicyError('Target is closed')

    if (!request.sessionId) return this.dispatchBrowserCommand(request.method, request.params)

    if (
      request.sessionId !== this.syntheticSessionId &&
      !this.childSessionIds.has(request.sessionId)
    ) {
      throw new CdpPolicyError('Unknown target session')
    }
    if (isForbiddenTargetCommand(request.method)) {
      throw new CdpPolicyError('Target enumeration or creation is not permitted')
    }
    if (request.method === 'Target.getTargetInfo') {
      this.assertSelectedTarget(request.params)
      return { targetInfo: this.targetInfo }
    }
    if (request.method === 'Target.getTargets') {
      return { targetInfos: [this.targetInfo] }
    }
    if (request.method === 'Target.setDiscoverTargets') return {}
    if (request.method.startsWith('Target.') && request.method !== 'Target.setAutoAttach') {
      throw new CdpPolicyError('Target access is not permitted')
    }
    if (request.method.startsWith('Browser.')) {
      throw new CdpPolicyError('Browser-wide commands are not permitted')
    }

    const childSessionId =
      request.sessionId === this.syntheticSessionId ? undefined : request.sessionId
    if (request.method === 'Target.setAutoAttach') {
      return await this.debuggerClient.sendCommand(
        request.method,
        withoutTargetPause(request.params),
        childSessionId
      )
    }
    if (request.method === 'Page.createIsolatedWorld' && childSessionId) {
      const ready = await this.childReadiness.get(childSessionId)?.promise
      if (ready !== true) throw new CdpPolicyError('frame_detached')
    }
    if (request.method === 'Input.insertText') {
      return await this.insertText(request.params, childSessionId)
    }
    if (request.method === 'Input.dispatchKeyEvent') {
      // Keep the exact admitted guest internally focused even when its containing window is not
      // foregrounded. This does not activate another Target or bring the application forward.
      this.guest.hostWebContents?.focus()
      this.guest.focus()
      if (
        (request.params?.type === 'char' || request.params?.type === 'keyDown') &&
        typeof request.params.text === 'string' &&
        request.params.text.length > 0
      ) {
        if (request.params.type === 'keyDown') {
          // Electron drops the text payload for guest keyDown. Forward the native key event with
          // every key/code/modifier/repeat field intact, but without its ineffective insertion
          // fields, then deliver exactly one insertion to the frame that owns focus. The later
          // native keyUp remains untouched.
          const result = await this.debuggerClient.sendCommand(
            request.method,
            withoutTextInsertion(request.params),
            childSessionId
          )
          await this.insertText({ text: request.params.text }, childSessionId)
          return result
        }
        return await this.insertText({ text: request.params.text }, childSessionId)
      }
    }
    const result = await this.debuggerClient.sendCommand(
      request.method,
      request.params,
      request.method === 'Page.createIsolatedWorld' ? undefined : childSessionId
    )
    return result
  }

  /**
   * Electron's debugger currently acknowledges `Input.insertText` for a webview guest but drops
   * the text, even when Playwright has focused the exact input. WebContents.insertText is the
   * narrow native compatibility path: it targets this exact admitted guest and follows the
   * renderer's focused frame, including an iframe/OOPIF. It does not execute page-provided code or
   * require access to the frame DOM. Remove it once Electron's guest debugger reliably implements
   * `Input.insertText`.
   */
  private async insertText(
    params: CdpParams | undefined,
    sessionId: string | undefined
  ): Promise<unknown> {
    if (
      !params ||
      Object.keys(params).length !== 1 ||
      typeof params.text !== 'string' ||
      Buffer.byteLength(params.text, 'utf8') > MAX_INSERT_TEXT_BYTES
    ) {
      throw new CdpPolicyError('Invalid text insertion request')
    }
    if (params.text.length === 0) return {}

    try {
      // An OOPIF has its own admitted child CDP session. Electron's WebContents.insertText can
      // crash that remote renderer when it owns focus, while the child Input domain targets it
      // precisely. Same-process frames have no child session, so use the exact guest's native
      // insertion API there; unlike a top-document Runtime.evaluate shim it follows frame focus.
      if (sessionId) {
        return await this.debuggerClient.sendCommand('Input.insertText', params, sessionId)
      }
      const focusedChildSession = await this.findFocusedChildSession()
      if (focusedChildSession) {
        return await this.debuggerClient.sendCommand(
          'Input.insertText',
          params,
          focusedChildSession
        )
      }
      this.guest.focus()
      await this.guest.insertText(params.text)
    } catch (error) {
      if (error instanceof CdpPolicyError) throw error
      throw new CdpPolicyError('frame_input_delivery_failed')
    }
    return {}
  }

  private async findFocusedChildSession(): Promise<string | undefined> {
    const probes = [...this.childSessionIds].map((sessionId): ChildFocusProbe | null => {
      const frameId = this.childFrameIds.get(sessionId)
      return frameId ? { sessionId, frameId } : null
    })
    if (probes.some((probe) => probe === null)) {
      // A live child without an exact navigated frame identity may own focus. Falling back to the
      // top document in that state could deliver text to the wrong renderer.
      throw new CdpPolicyError('frame_input_delivery_failed')
    }
    const focused = (
      await Promise.all(
        (probes as ChildFocusProbe[]).map(async (probe) => ({
          sessionId: probe.sessionId,
          focused: await withTimeout(
            this.probeChildFocusInIsolatedWorld(probe),
            FOCUS_PROBE_TIMEOUT_MS
          )
        }))
      )
    ).filter((probe) => probe.focused)
    if (focused.length > 1) throw new CdpPolicyError('frame_input_delivery_failed')
    return focused[0]?.sessionId
  }

  private async probeChildFocusInIsolatedWorld(probe: ChildFocusProbe): Promise<boolean> {
    try {
      // Never ask the page's main world whether it owns focus: page script can replace
      // document.hasFocus and redirect typed secrets into a malicious iframe. A named isolated
      // world has its own pristine JavaScript intrinsics while observing the same browser focus
      // state. Both commands use the exact admitted, ready child session and its last observed
      // frame identity; neither command can escape to another Target.
      const world = await this.debuggerClient.sendCommand(
        'Page.createIsolatedWorld',
        {
          frameId: probe.frameId,
          worldName: this.focusWorldName
        },
        probe.sessionId
      )
      const executionContextId =
        isRecord(world) &&
        typeof world.executionContextId === 'number' &&
        Number.isSafeInteger(world.executionContextId) &&
        world.executionContextId > 0
          ? world.executionContextId
          : undefined
      if (!executionContextId) throw new Error('focus-world-missing')
      const result = await this.debuggerClient.sendCommand(
        'Runtime.evaluate',
        {
          expression: 'Document.prototype.hasFocus.call(document) === true',
          contextId: executionContextId,
          returnByValue: true,
          silent: true
        },
        probe.sessionId
      )
      const evaluated =
        isRecord(result) && isRecord(result.result) ? result.result.value : undefined
      if (evaluated !== true && evaluated !== false) throw new Error('focus-result-invalid')
      return evaluated
    } catch {
      throw new CdpPolicyError('frame_input_delivery_failed')
    }
  }

  private async dispatchBrowserCommand(method: string, params?: CdpParams): Promise<unknown> {
    switch (method) {
      case 'Browser.getVersion':
        return this.browserVersion
      case 'Browser.setDownloadBehavior':
        if (params?.behavior !== 'deny') {
          throw new CdpPolicyError('Downloads are not permitted')
        }
        return {}
      case 'Target.setAutoAttach':
        if (params?.autoAttach !== true || params.flatten !== true) {
          throw new CdpPolicyError('Only flattened automatic attachment is permitted')
        }
        this.emitAttachedTarget()
        return {}
      case 'Target.getTargetInfo':
        this.assertSelectedTarget(params)
        return { targetInfo: this.targetInfo }
      case 'Target.getTargets':
        return { targetInfos: [this.targetInfo] }
      case 'Target.setDiscoverTargets':
        return {}
      default:
        if (isForbiddenTargetCommand(method) || method.startsWith('Browser.')) {
          throw new CdpPolicyError('Browser-wide target access is not permitted')
        }
        throw new CdpPolicyError('Unsupported browser-level command')
    }
  }

  private assertSelectedTarget(params?: CdpParams): void {
    const requestedTarget = params?.targetId
    if (requestedTarget !== undefined && requestedTarget !== this.targetInfo.targetId) {
      throw new CdpPolicyError('Target access is not permitted')
    }
  }

  private emitAttachedTarget(): void {
    if (this.emittedAttached) return
    this.emittedAttached = true
    this.emit({
      method: 'Target.attachedToTarget',
      params: {
        sessionId: this.syntheticSessionId,
        targetInfo: this.targetInfo,
        waitingForDebugger: false
      }
    })
  }

  private respond(
    request: CdpRequest,
    payload: { error?: { code: number; message: string }; result?: unknown }
  ): void {
    if (this.closed) return
    this.emit({
      id: request.id,
      ...(request.sessionId ? { sessionId: request.sessionId } : {}),
      ...payload
    })
  }

  private emit(message: object): void {
    if (!this.closed) this.messageHandler?.(message)
  }

  private enqueueEvent(message: object): void {
    if (this.closed || !this.messageHandler) return
    const now = Date.now()
    if (now - this.eventWindowStartedAt >= 1_000) {
      this.eventWindowStartedAt = now
      this.eventCount = 0
    }
    this.eventCount += 1
    if (this.eventCount > MAX_CDP_EVENTS_PER_SECOND) {
      this.terminate('cdp_event_rate_exceeded')
      return
    }
    if (this.eventQueue.length >= MAX_CDP_EVENT_QUEUE) {
      this.terminate('cdp_event_queue_exceeded')
      return
    }
    this.eventQueue.push(message)
    if (this.flushScheduled) return
    this.flushScheduled = true
    // Playwright's CDP transport contract observes debugger notifications synchronously relative
    // to command responses. Preserve that ordering; the queue only absorbs bounded re-entrant
    // delivery caused by a handler, while the rate gate bounds ordinary sequential storms.
    this.flushEventQueue()
  }

  private flushEventQueue(): void {
    try {
      while (!this.closed && this.eventQueue.length > 0) {
        const queued = this.eventQueue
        this.eventQueue = []
        for (const message of queued) {
          if (this.closed) return
          this.messageHandler?.(message)
        }
      }
    } finally {
      this.flushScheduled = false
      if (this.closed) this.eventQueue = []
    }
  }

  private terminate(reason: string): void {
    if (this.closed) return
    this.closed = true

    if (this.emittedAttached) {
      this.messageHandler?.({
        method: 'Target.detachedFromTarget',
        params: { sessionId: this.syntheticSessionId, targetId: this.targetInfo.targetId }
      })
    }

    this.debuggerClient.off('message', this.handleDebuggerMessage)
    this.debuggerClient.off('detach', this.handleDebuggerDetach)
    this.guest.removeListener('destroyed', this.handleGuestDestroyed)
    if (this.ownsDebugger && !this.guest.isDestroyed() && this.debuggerClient.isAttached()) {
      try {
        this.debuggerClient.detach()
      } catch {
        // The target may have closed between the liveness check and detach.
      }
    }
    this.ownsDebugger = false
    for (const readiness of this.childReadiness.values()) readiness.settle(false)
    this.childReadiness.clear()
    this.childSessionIds.clear()
    this.childFrameIds.clear()
    this.childTargetIds.clear()
    this.eventQueue = []
    this.handleClosed(this)
    this.closeReason = reason
    this.deliverCloseNotification()
  }

  private deliverCloseNotification(): void {
    if (
      this.closeNotificationDelivered ||
      this.closeReason === undefined ||
      this.closeHandler === undefined
    ) {
      return
    }
    this.closeNotificationDelivered = true
    this.closeHandler(this.closeReason)
  }
}

class CdpPolicyError extends Error {}

function createChildReadiness(): ChildReadiness {
  let settled = false
  let resolvePromise!: (ready: boolean) => void
  const promise = new Promise<boolean>((resolve) => {
    resolvePromise = resolve
  })
  return {
    promise,
    settle(ready) {
      if (settled) return
      settled = true
      resolvePromise(ready)
    }
  }
}

function resolvedChildReadiness(): ChildReadiness {
  return { promise: Promise.resolve(true), settle: () => undefined }
}

function isForbiddenTargetCommand(method: string): boolean {
  return (
    method === 'Target.attachToTarget' ||
    method === 'Target.closeTarget' ||
    method === 'Target.createBrowserContext' ||
    method === 'Target.createTarget' ||
    method === 'Target.disposeBrowserContext'
  )
}

function parseRequest(value: object): CdpRequest | null {
  if (!isRecord(value)) return null
  if (!Number.isSafeInteger(value.id) || (value.id as number) < 0) return null
  if (
    typeof value.method !== 'string' ||
    value.method.length === 0 ||
    value.method.length > MAX_METHOD_LENGTH
  ) {
    return null
  }
  const params = value.params
  if (params !== undefined && !isRecord(params)) return null
  const sessionId = value.sessionId
  if (
    sessionId !== undefined &&
    (typeof sessionId !== 'string' ||
      sessionId.length === 0 ||
      sessionId.length > MAX_SESSION_ID_LENGTH)
  ) {
    return null
  }

  const request: CdpRequest = { id: value.id as number, method: value.method }
  if (isRecord(params)) {
    try {
      request.params = cloneBoundedCdpRecord(params)
    } catch {
      return null
    }
  }
  if (typeof sessionId === 'string') request.sessionId = sessionId
  return request
}

function withoutTargetPause(params: CdpParams | undefined): CdpParams {
  if (!params || params.autoAttach !== true || params.flatten !== true) {
    throw new CdpPolicyError('Invalid target auto-attach request')
  }
  return { ...params, waitForDebuggerOnStart: false }
}

function withoutTextInsertion(params: CdpParams): CdpParams {
  const forwarded = { ...params }
  delete forwarded.text
  delete forwarded.unmodifiedText
  return forwarded
}

function cloneBoundedCdpRecord(value: Record<string, unknown>): CdpParams {
  const cloned = cloneBoundedCdpPayload(value)
  if (!isRecord(cloned)) throw new CdpPolicyError('Invalid CDP payload')
  return cloned
}

function cloneBoundedCdpPayload(value: unknown): unknown {
  let nodes = 0
  let encodedBytes = 0
  const ancestors = new WeakSet<object>()
  const addBytes = (amount: number): void => {
    encodedBytes += amount
    if (encodedBytes > MAX_CDP_PAYLOAD_BYTES) {
      throw new CdpPolicyError('CDP payload exceeds managed limits')
    }
  }
  const addString = (current: string, fieldLimit = MAX_CDP_VALUE_STRING_BYTES): void => {
    if (Buffer.byteLength(current, 'utf8') > fieldLimit) {
      throw new CdpPolicyError('CDP payload exceeds managed limits')
    }
    addBytes(jsonEncodedStringBytes(current))
  }
  const clone = (current: unknown, depth: number): unknown => {
    nodes += 1
    if (depth > MAX_CDP_PAYLOAD_DEPTH || nodes > MAX_CDP_PAYLOAD_NODES) {
      throw new CdpPolicyError('CDP payload exceeds managed limits')
    }
    if (current === null) {
      addBytes(4)
      return null
    }
    if (typeof current === 'string') {
      addString(current)
      return current
    }
    if (typeof current === 'boolean') {
      addBytes(current ? 4 : 5)
      return current
    }
    if (typeof current === 'number') {
      if (!Number.isFinite(current)) throw new CdpPolicyError('Invalid CDP payload')
      addBytes(String(current).length)
      return current
    }
    if (typeof current !== 'object') throw new CdpPolicyError('Invalid CDP payload')
    if (ancestors.has(current)) throw new CdpPolicyError('Invalid CDP payload')
    ancestors.add(current)
    try {
      if (Array.isArray(current)) {
        if (current.length > MAX_CDP_CONTAINER_ITEMS) {
          throw new CdpPolicyError('CDP payload exceeds managed limits')
        }
        addBytes(2 + Math.max(0, current.length - 1))
        const copy: unknown[] = []
        for (const child of current) {
          if (child === undefined) {
            nodes += 1
            if (nodes > MAX_CDP_PAYLOAD_NODES) {
              throw new CdpPolicyError('CDP payload exceeds managed limits')
            }
            addBytes(4)
            copy.push(null)
          } else {
            copy.push(clone(child, depth + 1))
          }
        }
        return copy
      }
      const prototype = Object.getPrototypeOf(current)
      if (prototype !== Object.prototype && prototype !== null) {
        throw new CdpPolicyError('Invalid CDP payload')
      }
      const copy: Record<string, unknown> = {}
      let properties = 0
      let includedProperties = 0
      addBytes(2)
      for (const key in current) {
        if (!Object.hasOwn(current, key)) continue
        properties += 1
        if (properties > MAX_CDP_OBJECT_PROPERTIES) {
          throw new CdpPolicyError('CDP payload exceeds managed limits')
        }
        const descriptor = Object.getOwnPropertyDescriptor(current, key)
        if (!descriptor || descriptor.get || descriptor.set) {
          throw new CdpPolicyError('Invalid CDP payload')
        }
        // ConnectOverCDP normally crosses a JSON wire. Preserve JSON's treatment of optional
        // object properties so Playwright may pass `{ optional: undefined }` without widening the
        // accepted value domain sent to Electron.
        if (descriptor.value === undefined) continue
        includedProperties += 1
        addString(key, MAX_CDP_PROPERTY_NAME_BYTES)
        addBytes(includedProperties === 1 ? 1 : 2)
        copy[key] = clone(descriptor.value, depth + 1)
      }
      return copy
    } finally {
      ancestors.delete(current)
    }
  }
  return clone(value, 0)
}

function jsonEncodedStringBytes(value: string): number {
  let bytes = 2
  for (let index = 0; index < value.length;) {
    const codePoint = value.codePointAt(index)
    if (codePoint === undefined) break
    const width = codePoint > 0xffff ? 2 : 1
    const loneSurrogate = width === 1 && codePoint >= 0xd800 && codePoint <= 0xdfff
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
    } else if (codePoint < 0x20 || loneSurrogate) {
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

function normalizeBrowserVersion(value: unknown): CdpParams {
  if (!isRecord(value)) {
    return {
      jsVersion: '',
      product: 'Chrome/0.0.0.0',
      protocolVersion: '1.3',
      revision: '',
      userAgent: 'MyCopilot managed browser target'
    }
  }
  return {
    jsVersion: boundedString(value.jsVersion),
    product: boundedString(value.product) || 'Chrome/0.0.0.0',
    protocolVersion: boundedString(value.protocolVersion) || '1.3',
    revision: boundedString(value.revision),
    userAgent: boundedString(value.userAgent) || 'MyCopilot managed browser target'
  }
}

function normalizeTargetInfo(value: unknown, fallback: CdpTargetInfo): CdpTargetInfo {
  if (!isRecord(value) || !isRecord(value.targetInfo)) return fallback
  const targetInfo = value.targetInfo
  return {
    ...fallback,
    browserContextId: boundedString(targetInfo.browserContextId) || fallback.browserContextId,
    targetId: boundedString(targetInfo.targetId) || fallback.targetId,
    title: boundedString(targetInfo.title) || fallback.title,
    url: boundedString(targetInfo.url) || fallback.url
  }
}

function boundedString(value: unknown): string {
  return typeof value === 'string' ? value.slice(0, MAX_CDP_STRING_LENGTH) : ''
}

async function withTimeout<T>(operation: Promise<T>, timeoutMs: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new CdpPolicyError('frame_input_delivery_failed')), timeoutMs)
  })
  try {
    return await Promise.race([operation, timeout])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
