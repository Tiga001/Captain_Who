import { randomUUID } from 'node:crypto'
import type { ConnectOverCDPTransport } from 'playwright'
import type { Debugger, Event as ElectronEvent, WebContents } from 'electron'
import { GuestPdfError } from './ElectronGuestPdfPrinter'
import { ElectronGuestPdfStream } from './ElectronGuestPdfStream'

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

/**
 * Main-only identity used by the private SurfaceGroup compositor. The values are synthetic to
 * this one admitted guest transport and must never cross Renderer IPC.
 */
export interface ElectronGuestCdpIdentity {
  browserContextId: string
  sessionId: string
  targetId: string
  targetInfo: Readonly<CdpTargetInfo>
}

export interface ElectronGuestCdpPresentation {
  title: string
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
const MAX_ARTIFACT_BINARY_BYTES = 64 * 1_024 * 1_024
const MAX_ARTIFACT_BASE64_BYTES = Math.ceil(MAX_ARTIFACT_BINARY_BYTES / 3) * 4
const MAX_ARTIFACT_CDP_PAYLOAD_BYTES = MAX_ARTIFACT_BASE64_BYTES + 1 * 1_024 * 1_024
const MAX_CDP_EVENT_QUEUE = 1_024
const MAX_CDP_EVENTS_PER_SECOND = 4_096
const MAX_CHILD_SESSIONS = 64
const FOCUS_PROBE_TIMEOUT_MS = 500
const MAX_MANAGED_COOKIES = 512
const MAX_MANAGED_COOKIE_BYTES = 1024 * 1024
const MAX_MANAGED_COOKIE_FIELD_BYTES = 64 * 1024
const MAX_MANAGED_COOKIE_URL_BYTES = 8 * 1024

export class ElectronGuestCdpTransport implements ConnectOverCDPTransport {
  private readonly childSessionIds = new Set<string>()
  private readonly childReadiness = new Map<string, ChildReadiness>()
  private readonly childFrameIds = new Map<string, string>()
  private readonly childOwnerFrameIds = new Map<string, string>()
  private readonly childParentSessions = new Map<string, string>()
  private readonly childTargetIds = new Map<string, string>()
  private readonly childTargetTypes = new Map<string, 'iframe' | 'worker'>()
  private readonly debuggerClient: Debugger
  private readonly focusWorldName = `mycopilot-focus-${randomUUID()}`
  private readonly syntheticSessionId = `mycopilot-page-${randomUUID()}`
  private targetInfo: CdpTargetInfo
  private browserVersion: CdpParams = {
    jsVersion: '',
    product: 'Chrome/0.0.0.0',
    protocolVersion: '1.3',
    revision: '',
    userAgent: 'CaptainWho managed browser target'
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
  private readonly pdfStream: ElectronGuestPdfStream

  constructor(
    private readonly guest: WebContents,
    private readonly handleClosed: (transport: ElectronGuestCdpTransport) => void,
    private readonly resolvePresentation?: (
      physicalUrl: string
    ) => ElectronGuestCdpPresentation | null
  ) {
    this.pdfStream = new ElectronGuestPdfStream(guest)
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

  managedIdentity(): ElectronGuestCdpIdentity {
    const targetInfo = this.presentedTargetInfo()
    return {
      browserContextId: targetInfo.browserContextId,
      sessionId: this.syntheticSessionId,
      targetId: targetInfo.targetId,
      targetInfo
    }
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
      safeParams = isRecord(params)
        ? cloneBoundedCdpRecord(params, cdpPayloadBudget(method, 'event'))
        : {}
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
      this.childTargetTypes.set(attached.sessionId, attached.type)
      this.childParentSessions.set(attached.sessionId, sessionId)
      if (attached.ownerFrameId) {
        this.childOwnerFrameIds.set(attached.sessionId, attached.ownerFrameId)
      }
      this.childReadiness.set(
        attached.sessionId,
        attached.type === 'worker' || !attached.waitingForDebugger
          ? resolvedChildReadiness()
          : createChildReadiness()
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

    if (mappedSessionId === this.syntheticSessionId) {
      safeParams = this.sanitizeTopLevelEvent(method, safeParams)
    }
    const event = { method, params: safeParams, sessionId: mappedSessionId }
    this.enqueueEvent(event)

    if (method === 'Target.detachedFromTarget') {
      const childSessionId = safeParams.sessionId
      if (typeof childSessionId === 'string') {
        this.retireChildSession(childSessionId)
      }
    }
  }

  private validateAttachedChild(
    params: CdpParams,
    parentSessionId: string
  ): {
    ownerFrameId?: string
    sessionId: string
    targetId: string
    type: 'iframe' | 'worker'
    waitingForDebugger: boolean
  } | null {
    const childSessionId = params.sessionId
    const targetInfo = params.targetInfo
    if (
      typeof childSessionId !== 'string' ||
      childSessionId.length === 0 ||
      childSessionId.length > MAX_SESSION_ID_LENGTH ||
      !isRecord(targetInfo) ||
      (targetInfo.type !== 'iframe' && targetInfo.type !== 'worker') ||
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
    const expectedOwnerFrame = parentSessionId
      ? this.childTargetTypes.get(parentSessionId) === 'iframe'
        ? this.childTargetIds.get(parentSessionId)
        : this.childOwnerFrameIds.get(parentSessionId)
      : this.targetInfo.targetId
    if (!expectedParentTarget) return null
    if (
      (targetInfo.type === 'iframe' && targetInfo.parentFrameId !== expectedParentTarget) ||
      (targetInfo.type === 'worker' &&
        (!expectedOwnerFrame ||
          (targetInfo.parentFrameId !== undefined &&
            targetInfo.parentFrameId !== expectedOwnerFrame)))
    ) {
      return null
    }
    if (
      this.childSessionIds.has(childSessionId) ||
      [...this.childTargetIds.values()].includes(targetInfo.targetId)
    ) {
      return null
    }
    return {
      ownerFrameId:
        targetInfo.type === 'iframe' ? targetInfo.targetId : (expectedOwnerFrame as string),
      sessionId: childSessionId,
      targetId: targetInfo.targetId,
      type: targetInfo.type,
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
      const boundedResult = cloneBoundedCdpPayload(
        rawResult === undefined ? {} : rawResult,
        cdpPayloadBudget(request.method, 'response')
      )
      const result = this.sanitizeCommandResult(request, boundedResult)
      this.respond(request, { result })
    } catch (error) {
      this.respond(request, {
        error: {
          code: -32_000,
          message:
            error instanceof CdpPolicyError || error instanceof GuestPdfError
              ? error.message
              : 'Managed target command failed'
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
    const childSessionId =
      request.sessionId === this.syntheticSessionId ? undefined : request.sessionId
    if (request.method === 'Target.detachFromTarget') {
      const detachedSessionId = request.params?.sessionId
      if (
        !request.params ||
        !hasExactOwnKeys(request.params, ['sessionId']) ||
        typeof detachedSessionId !== 'string' ||
        !this.childSessionIds.has(detachedSessionId) ||
        this.childParentSessions.get(detachedSessionId) !== (childSessionId ?? '')
      ) {
        throw new CdpPolicyError('Unknown target session')
      }
      return await this.debuggerClient.sendCommand(request.method, request.params, childSessionId)
    }
    if (isForbiddenTargetCommand(request.method)) {
      throw new CdpPolicyError('Target enumeration or creation is not permitted')
    }
    if (request.method === 'Target.getTargetInfo') {
      this.assertSelectedTarget(request.params)
      return { targetInfo: this.presentedTargetInfo() }
    }
    if (request.method === 'Target.getTargets') {
      return { targetInfos: [this.presentedTargetInfo()] }
    }
    if (request.method === 'Target.setDiscoverTargets') return {}
    if (request.method.startsWith('Target.') && request.method !== 'Target.setAutoAttach') {
      throw new CdpPolicyError('Target access is not permitted')
    }
    if (request.method.startsWith('Browser.')) {
      throw new CdpPolicyError('Browser-wide commands are not permitted')
    }
    if (request.method.startsWith('Storage.')) {
      throw new CdpPolicyError('Storage commands require the managed BrowserContext session')
    }

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
    if (request.method === 'Page.printToPDF') {
      if (childSessionId)
        throw new CdpPolicyError('PDF printing requires the admitted page session')
      return await this.pdfStream.print(request.params)
    }
    if (
      (request.method === 'IO.read' || request.method === 'IO.close') &&
      this.pdfStream.ownsHandle(request.params)
    ) {
      if (childSessionId)
        throw new CdpPolicyError('PDF streams belong to the admitted page session')
      return request.method === 'IO.read'
        ? this.pdfStream.read(request.params)
        : this.pdfStream.closeStream(request.params)
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
    const probes = [...this.childSessionIds]
      .filter((sessionId) => this.childTargetTypes.get(sessionId) === 'iframe')
      .map((sessionId): ChildFocusProbe | null => {
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
    if (focused.length <= 1) return focused[0]?.sessionId
    const deepest = focused.filter(
      (candidate) =>
        !focused.some(
          (other) =>
            other.sessionId !== candidate.sessionId &&
            this.isChildSessionAncestor(candidate.sessionId, other.sessionId)
        )
    )
    if (deepest.length !== 1) throw new CdpPolicyError('frame_input_delivery_failed')
    return deepest[0]?.sessionId
  }

  private isChildSessionAncestor(ancestor: string, descendant: string): boolean {
    let current = this.childParentSessions.get(descendant)
    const visited = new Set<string>()
    while (current) {
      if (current === ancestor) return true
      if (visited.has(current)) return false
      visited.add(current)
      current = this.childParentSessions.get(current)
    }
    return false
  }

  private retireChildSession(sessionId: string): void {
    for (const [childSessionId, parentSessionId] of [...this.childParentSessions]) {
      if (parentSessionId === sessionId) this.retireChildSession(childSessionId)
    }
    this.childSessionIds.delete(sessionId)
    this.childFrameIds.delete(sessionId)
    this.childOwnerFrameIds.delete(sessionId)
    this.childParentSessions.delete(sessionId)
    this.childTargetIds.delete(sessionId)
    this.childTargetTypes.delete(sessionId)
    this.childReadiness.get(sessionId)?.settle(false)
    this.childReadiness.delete(sessionId)
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
      case 'Storage.getCookies':
      case 'Storage.setCookies':
      case 'Storage.clearCookies':
        return await this.dispatchManagedCookieCommand(method, params)
      case 'Target.setAutoAttach':
        if (params?.autoAttach !== true || params.flatten !== true) {
          throw new CdpPolicyError('Only flattened automatic attachment is permitted')
        }
        this.emitAttachedTarget()
        return {}
      case 'Target.getTargetInfo':
        this.assertSelectedTarget(params)
        return { targetInfo: this.presentedTargetInfo() }
      case 'Target.getTargets':
        return { targetInfos: [this.presentedTargetInfo()] }
      case 'Target.setDiscoverTargets':
        return {}
      default:
        if (isForbiddenTargetCommand(method) || method.startsWith('Browser.')) {
          throw new CdpPolicyError('Browser-wide target access is not permitted')
        }
        throw new CdpPolicyError('Unsupported browser-level command')
    }
  }

  private async dispatchManagedCookieCommand(
    method: 'Storage.getCookies' | 'Storage.setCookies' | 'Storage.clearCookies',
    params: CdpParams | undefined
  ): Promise<unknown> {
    const contextProvided = params?.browserContextId !== undefined
    const expectedKeys = method === 'Storage.setCookies' ? ['cookies'] : []
    const expectedKeysWithContext = [...expectedKeys, 'browserContextId']
    if (
      !params ||
      (!hasExactOwnKeys(params, expectedKeys) &&
        !hasExactOwnKeys(params, expectedKeysWithContext)) ||
      (contextProvided && params.browserContextId !== this.targetInfo.browserContextId)
    ) {
      throw new CdpPolicyError('Cookie command is not bound to the managed BrowserContext')
    }
    if (method === 'Storage.setCookies') validateManagedCookieParams(params.cookies)
    // Playwright knows only the Host-owned synthetic BrowserContext identity. Chromium does not:
    // omitting its optional browserContextId makes the command act on this exact debugger guest's
    // isolated Electron partition, while the equality check above prevents a caller from widening
    // the synthetic authority presented by this transport.
    // Electron's WebContents debugger is a page session rather than Chromium's browser session;
    // Storage.* is rejected there. Network's cookie trio has the same CookieParam/Cookie result
    // shape and operates on this exact admitted guest's isolated partition.
    const networkMethod =
      method === 'Storage.getCookies'
        ? 'Network.getAllCookies'
        : method === 'Storage.setCookies'
          ? 'Network.setCookies'
          : 'Network.clearBrowserCookies'
    const result = await this.debuggerClient.sendCommand(
      networkMethod,
      method === 'Storage.setCookies' ? { cookies: params.cookies } : {}
    )
    if (method === 'Storage.getCookies') validateManagedCookieResult(result)
    return result
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
        targetInfo: this.presentedTargetInfo(),
        waitingForDebugger: false
      }
    })
  }

  private presentedTargetInfo(physicalUrl = this.guest.getURL()): CdpTargetInfo {
    const presentation = this.safePresentation(physicalUrl)
    return presentation
      ? {
          ...this.targetInfo,
          title: presentation.title,
          url: presentation.url
        }
      : {
          ...this.targetInfo,
          title: boundedString(this.guest.getTitle()),
          url: publicCdpUrl(physicalUrl)
        }
  }

  private safePresentation(physicalUrl: string): ElectronGuestCdpPresentation | null {
    try {
      const presentation = this.resolvePresentation?.(physicalUrl)
      if (!presentation) return null
      const parsed = new URL(presentation.url)
      if (
        !['http:', 'https:'].includes(parsed.protocol) ||
        parsed.username !== '' ||
        parsed.password !== ''
      ) {
        return null
      }
      return {
        title: boundedString(presentation.title),
        url: boundedString(parsed.toString())
      }
    } catch {
      return null
    }
  }

  private logicalUrl(physicalUrl: unknown): unknown {
    if (typeof physicalUrl !== 'string') return physicalUrl
    const publicUrl = publicHttpUrl(physicalUrl)
    if (publicUrl) return publicUrl
    return this.safePresentation(physicalUrl)?.url ?? 'about:blank'
  }

  private logicalOrigin(physicalOrigin: unknown): unknown {
    if (typeof physicalOrigin !== 'string') return physicalOrigin
    const publicOrigin = publicHttpOrigin(physicalOrigin)
    if (publicOrigin) return publicOrigin
    const presentation = this.safePresentation(this.guest.getURL())
    return presentation && haveSameProtocolAndHost(physicalOrigin, this.guest.getURL())
      ? new URL(presentation.url).origin
      : ''
  }

  private sanitizeTopLevelEvent(method: string, params: CdpParams): CdpParams {
    if (method === 'Page.frameNavigated' && isRecord(params.frame)) {
      return { ...params, frame: this.sanitizeFrameRecord(params.frame) }
    }
    if (
      (method === 'Page.navigatedWithinDocument' ||
        method === 'Page.frameRequestedNavigation' ||
        method === 'Page.frameStartedNavigating' ||
        method === 'Page.downloadWillBegin' ||
        method === 'Page.windowOpen' ||
        method === 'Network.webSocketCreated' ||
        method === 'Network.webTransportCreated') &&
      typeof params.url === 'string'
    ) {
      return { ...params, url: this.logicalUrl(params.url) }
    }
    if (method === 'Network.requestWillBeSent') {
      return {
        ...params,
        ...(typeof params.documentURL === 'string'
          ? { documentURL: this.logicalUrl(params.documentURL) }
          : {}),
        ...(isRecord(params.request) ? { request: this.sanitizeUrlRecord(params.request) } : {}),
        ...(isRecord(params.redirectResponse)
          ? { redirectResponse: this.sanitizeUrlRecord(params.redirectResponse) }
          : {})
      }
    }
    if (method === 'Network.responseReceived' && isRecord(params.response)) {
      return { ...params, response: this.sanitizeUrlRecord(params.response) }
    }
    if (method === 'Security.certificateError' && typeof params.requestURL === 'string') {
      return { ...params, requestURL: this.logicalUrl(params.requestURL) }
    }
    if (method === 'Runtime.executionContextCreated' && isRecord(params.context)) {
      return {
        ...params,
        context: {
          ...params.context,
          origin: this.logicalOrigin(params.context.origin)
        }
      }
    }
    if (
      (method === 'Debugger.scriptParsed' || method === 'Debugger.scriptFailedToParse') &&
      typeof params.url === 'string'
    ) {
      return { ...params, url: this.logicalUrl(params.url) }
    }
    if (method === 'Runtime.exceptionThrown' && isRecord(params.exceptionDetails)) {
      return {
        ...params,
        exceptionDetails: this.sanitizeUrlRecord(params.exceptionDetails)
      }
    }
    if (method === 'Log.entryAdded' && isRecord(params.entry)) {
      return { ...params, entry: this.sanitizeUrlRecord(params.entry) }
    }
    if (method === 'Target.targetInfoChanged' && isRecord(params.targetInfo)) {
      const targetInfo = params.targetInfo
      if (targetInfo.targetId === this.targetInfo.targetId) {
        return { ...params, targetInfo: this.presentedTargetInfo(String(targetInfo.url ?? '')) }
      }
    }
    return params
  }

  private sanitizeUrlRecord(record: CdpParams): CdpParams {
    return typeof record.url === 'string' ? { ...record, url: this.logicalUrl(record.url) } : record
  }

  private sanitizeFrameRecord(record: CdpParams): CdpParams {
    const url = this.logicalUrl(record.url)
    const logicalUrl = typeof url === 'string' ? publicHttpUrl(url) : null
    return {
      ...record,
      ...(record.url !== undefined ? { url } : {}),
      ...(record.unreachableUrl !== undefined
        ? { unreachableUrl: this.logicalUrl(record.unreachableUrl) }
        : {}),
      ...(record.securityOrigin !== undefined
        ? { securityOrigin: this.logicalOrigin(record.securityOrigin) }
        : {}),
      ...(record.domainAndRegistry !== undefined
        ? { domainAndRegistry: logicalUrl ? new URL(logicalUrl).hostname : '' }
        : {})
    }
  }

  private sanitizeCommandResult(request: CdpRequest, result: unknown): unknown {
    if (!isRecord(result)) return result
    if (request.method === 'Page.getNavigationHistory' && Array.isArray(result.entries)) {
      return {
        ...result,
        entries: result.entries.map((entry) =>
          isRecord(entry) ? { ...entry, url: this.logicalUrl(entry.url) } : entry
        )
      }
    }
    if (
      (request.method === 'Page.getFrameTree' || request.method === 'Page.getResourceTree') &&
      isRecord(result.frameTree)
    ) {
      return { ...result, frameTree: this.sanitizeFrameTree(result.frameTree) }
    }
    return result
  }

  private sanitizeFrameTree(value: CdpParams): CdpParams {
    const frame = isRecord(value.frame) ? this.sanitizeFrameRecord(value.frame) : value.frame
    const childFrames = Array.isArray(value.childFrames)
      ? value.childFrames.map((child) => (isRecord(child) ? this.sanitizeFrameTree(child) : child))
      : value.childFrames
    const resources = Array.isArray(value.resources)
      ? value.resources.map((resource) =>
          isRecord(resource) ? this.sanitizeUrlRecord(resource) : resource
        )
      : value.resources
    return {
      ...value,
      frame,
      ...(childFrames ? { childFrames } : {}),
      ...(resources ? { resources } : {})
    }
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
    this.pdfStream.dispose()

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
    this.childOwnerFrameIds.clear()
    this.childParentSessions.clear()
    this.childTargetIds.clear()
    this.childTargetTypes.clear()
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

const MANAGED_COOKIE_PARAM_KEYS = new Set([
  'domain',
  'expires',
  'httpOnly',
  'name',
  'partitionKey',
  'path',
  'priority',
  'sameSite',
  'secure',
  'sourcePort',
  'sourceScheme',
  'url',
  'value'
])

function validateManagedCookieParams(value: unknown): void {
  if (
    !Array.isArray(value) ||
    value.length > MAX_MANAGED_COOKIES ||
    encodedJsonBytes(value) > MAX_MANAGED_COOKIE_BYTES
  ) {
    throw new CdpPolicyError('Managed cookie payload exceeds its limit')
  }
  for (const cookie of value) validateManagedCookieParam(cookie)
}

function validateManagedCookieParam(value: unknown): void {
  if (
    !isRecord(value) ||
    Object.keys(value).some((key) => !MANAGED_COOKIE_PARAM_KEYS.has(key)) ||
    !boundedCookieString(value.name, MAX_MANAGED_COOKIE_FIELD_BYTES) ||
    !boundedCookieString(value.value, MAX_MANAGED_COOKIE_FIELD_BYTES) ||
    !optionalBoundedCookieString(value.url, MAX_MANAGED_COOKIE_URL_BYTES) ||
    !optionalBoundedCookieString(value.domain, MAX_MANAGED_COOKIE_URL_BYTES) ||
    !optionalBoundedCookieString(value.path, MAX_MANAGED_COOKIE_URL_BYTES) ||
    (value.secure !== undefined && typeof value.secure !== 'boolean') ||
    (value.httpOnly !== undefined && typeof value.httpOnly !== 'boolean') ||
    (value.sameSite !== undefined &&
      value.sameSite !== 'Strict' &&
      value.sameSite !== 'Lax' &&
      value.sameSite !== 'None') ||
    (value.expires !== undefined &&
      (typeof value.expires !== 'number' || !Number.isFinite(value.expires))) ||
    (value.priority !== undefined &&
      value.priority !== 'Low' &&
      value.priority !== 'Medium' &&
      value.priority !== 'High') ||
    (value.sourceScheme !== undefined &&
      value.sourceScheme !== 'Unset' &&
      value.sourceScheme !== 'NonSecure' &&
      value.sourceScheme !== 'Secure') ||
    (value.sourcePort !== undefined &&
      (!Number.isSafeInteger(value.sourcePort) ||
        (Number(value.sourcePort) !== -1 &&
          (Number(value.sourcePort) < 1 || Number(value.sourcePort) > 65_535)))) ||
    !validManagedCookiePartitionKey(value.partitionKey)
  ) {
    throw new CdpPolicyError('Invalid managed cookie payload')
  }
}

function validManagedCookiePartitionKey(value: unknown): boolean {
  if (value === undefined) return true
  return (
    isRecord(value) &&
    hasExactOwnKeys(value, ['hasCrossSiteAncestor', 'topLevelSite']) &&
    boundedCookieString(value.topLevelSite, MAX_MANAGED_COOKIE_URL_BYTES) &&
    typeof value.hasCrossSiteAncestor === 'boolean'
  )
}

function validateManagedCookieResult(value: unknown): void {
  if (!isRecord(value) || !hasExactOwnKeys(value, ['cookies']) || !Array.isArray(value.cookies)) {
    throw new CdpPolicyError('Invalid managed cookie response')
  }
  if (
    value.cookies.length > MAX_MANAGED_COOKIES ||
    value.cookies.some((cookie) => !isRecord(cookie)) ||
    encodedJsonBytes(value.cookies) > MAX_MANAGED_COOKIE_BYTES
  ) {
    throw new CdpPolicyError('Managed cookie response exceeds its limit')
  }
}

function boundedCookieString(value: unknown, maxBytes: number): value is string {
  return typeof value === 'string' && Buffer.byteLength(value, 'utf8') <= maxBytes
}

function optionalBoundedCookieString(value: unknown, maxBytes: number): boolean {
  return value === undefined || boundedCookieString(value, maxBytes)
}

function encodedJsonBytes(value: unknown): number {
  try {
    return Buffer.byteLength(JSON.stringify(value), 'utf8')
  } catch {
    throw new CdpPolicyError('Invalid managed cookie payload')
  }
}

function hasExactOwnKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
  const actual = Object.keys(value).sort()
  const sortedExpected = [...expected].sort()
  return (
    actual.length === sortedExpected.length &&
    actual.every((key, index) => key === sortedExpected[index])
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

interface CdpPayloadBudget {
  maxContainerItems?: number
  maxNodes?: number
  maxPayloadBytes: number
  maxStringBytes: number
  tooLargeMessage: string
}

function cdpPayloadBudget(
  method: string,
  direction: 'event' | 'response'
): CdpPayloadBudget | undefined {
  const artifactBearing =
    (direction === 'response' &&
      (method === 'Page.captureScreenshot' ||
        method === 'Network.getResponseBody' ||
        method === 'IO.read')) ||
    (direction === 'event' && method === 'Page.screencastFrame')
  if (artifactBearing) {
    return {
      maxPayloadBytes: MAX_ARTIFACT_CDP_PAYLOAD_BYTES,
      maxStringBytes: MAX_ARTIFACT_BASE64_BYTES,
      tooLargeMessage: 'artifact_too_large'
    }
  }
  if (
    direction === 'response' &&
    (method === 'Accessibility.getFullAXTree' ||
      method === 'Accessibility.getPartialAXTree' ||
      method === 'Accessibility.queryAXTree')
  ) {
    return {
      maxContainerItems: 65_536,
      maxNodes: 262_144,
      maxPayloadBytes: MAX_CDP_PAYLOAD_BYTES,
      maxStringBytes: MAX_CDP_VALUE_STRING_BYTES,
      tooLargeMessage: 'output_too_large'
    }
  }
  return undefined
}

function cloneBoundedCdpRecord(
  value: Record<string, unknown>,
  budget?: CdpPayloadBudget
): CdpParams {
  const cloned = cloneBoundedCdpPayload(value, budget)
  if (!isRecord(cloned)) throw new CdpPolicyError('Invalid CDP payload')
  return cloned
}

function cloneBoundedCdpPayload(value: unknown, budget?: CdpPayloadBudget): unknown {
  let nodes = 0
  let encodedBytes = 0
  const maxPayloadBytes = budget?.maxPayloadBytes ?? MAX_CDP_PAYLOAD_BYTES
  const maxStringBytes = budget?.maxStringBytes ?? MAX_CDP_VALUE_STRING_BYTES
  const maxNodes = budget?.maxNodes ?? MAX_CDP_PAYLOAD_NODES
  const maxContainerItems = budget?.maxContainerItems ?? MAX_CDP_CONTAINER_ITEMS
  const tooLargeMessage = budget?.tooLargeMessage ?? 'CDP payload exceeds managed limits'
  const ancestors = new WeakSet<object>()
  const addBytes = (amount: number): void => {
    encodedBytes += amount
    if (encodedBytes > maxPayloadBytes) {
      throw new CdpPolicyError(tooLargeMessage)
    }
  }
  const addString = (current: string, fieldLimit = maxStringBytes): void => {
    if (Buffer.byteLength(current, 'utf8') > fieldLimit) {
      throw new CdpPolicyError(tooLargeMessage)
    }
    addBytes(jsonEncodedStringBytes(current))
  }
  const clone = (current: unknown, depth: number): unknown => {
    nodes += 1
    if (depth > MAX_CDP_PAYLOAD_DEPTH || nodes > maxNodes) {
      throw new CdpPolicyError(tooLargeMessage)
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
        if (current.length > maxContainerItems) {
          throw new CdpPolicyError(tooLargeMessage)
        }
        addBytes(2 + Math.max(0, current.length - 1))
        const copy: unknown[] = []
        for (const child of current) {
          if (child === undefined) {
            nodes += 1
            if (nodes > maxNodes) {
              throw new CdpPolicyError(tooLargeMessage)
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
          throw new CdpPolicyError(tooLargeMessage)
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
      userAgent: 'CaptainWho managed browser target'
    }
  }
  return {
    jsVersion: boundedString(value.jsVersion),
    product: boundedString(value.product) || 'Chrome/0.0.0.0',
    protocolVersion: boundedString(value.protocolVersion) || '1.3',
    revision: boundedString(value.revision),
    userAgent: boundedString(value.userAgent) || 'CaptainWho managed browser target'
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

function publicHttpUrl(value: string): string | null {
  try {
    const parsed = new URL(value)
    if (
      !['http:', 'https:'].includes(parsed.protocol) ||
      parsed.username !== '' ||
      parsed.password !== ''
    ) {
      return null
    }
    return boundedString(parsed.toString())
  } catch {
    return null
  }
}

function publicHttpOrigin(value: string): string | null {
  const publicUrl = publicHttpUrl(value)
  return publicUrl ? new URL(publicUrl).origin : null
}

function haveSameProtocolAndHost(left: string, right: string): boolean {
  try {
    const leftUrl = new URL(left)
    const rightUrl = new URL(right)
    return leftUrl.protocol === rightUrl.protocol && leftUrl.host === rightUrl.host
  } catch {
    return false
  }
}

function publicCdpUrl(value: string): string {
  if (value === 'about:blank') return value
  return publicHttpUrl(value) ?? 'about:blank'
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
