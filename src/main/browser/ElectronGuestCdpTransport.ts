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

const MAX_METHOD_LENGTH = 256
const MAX_SESSION_ID_LENGTH = 512
const MAX_CDP_STRING_LENGTH = 4_096

export class ElectronGuestCdpTransport implements ConnectOverCDPTransport {
  private readonly childSessionIds = new Set<string>()
  private readonly debuggerClient: Debugger
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

    const safeParams = isRecord(params) ? params : {}
    if (method === 'Target.attachedToTarget') {
      const childSessionId = safeParams.sessionId
      if (
        typeof childSessionId !== 'string' ||
        childSessionId.length === 0 ||
        childSessionId.length > MAX_SESSION_ID_LENGTH
      ) {
        return
      }
      this.childSessionIds.add(childSessionId)
    }

    const mappedSessionId = sessionId || this.syntheticSessionId
    if (mappedSessionId !== this.syntheticSessionId && !this.childSessionIds.has(mappedSessionId)) {
      return
    }

    this.emit({ method, params: safeParams, sessionId: mappedSessionId })

    if (method === 'Target.detachedFromTarget') {
      const childSessionId = safeParams.sessionId
      if (typeof childSessionId === 'string') this.childSessionIds.delete(childSessionId)
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
      const result = await this.dispatchCommand(request)
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
    return await this.debuggerClient.sendCommand(request.method, request.params, childSessionId)
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
    this.childSessionIds.clear()
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
  if (isRecord(params)) request.params = params
  if (typeof sessionId === 'string') request.sessionId = sessionId
  return request
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
