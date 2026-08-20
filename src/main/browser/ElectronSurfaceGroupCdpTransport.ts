import { randomUUID } from 'node:crypto'
import type { ConnectOverCDPTransport } from 'playwright'
import type {
  ElectronGuestCdpIdentity,
  ElectronGuestCdpTransport
} from './ElectronGuestCdpTransport'

type CdpParams = Record<string, unknown>

interface CdpRequest {
  id: number
  method: string
  params?: CdpParams
  sessionId?: string
}

interface DelegateReply {
  error?: { code?: unknown; message?: unknown }
  result?: unknown
}

interface DelegatePending {
  reject: (error: Error) => void
  resolve: (reply: DelegateReply) => void
}

interface ContextControlPending {
  cancel(reason: string): void
  transport: ElectronGuestCdpTransport
}

interface GroupTargetInfo extends CdpParams {
  attached: true
  browserContextId: string
  canAccessOpener: false
  targetId: string
  title: string
  type: 'page'
  url: string
}

interface SurfaceEntry {
  childOutwardSessionByRaw: Map<string, string>
  childOutwardTargetByRaw: Map<string, string>
  childParentRawSessionByRaw: Map<string, string>
  childRawSessionByOutward: Map<string, string>
  childRawTargetBySession: Map<string, string>
  childTargetInfoByRawSession: Map<string, CdpParams>
  delegatePending: Map<number, DelegatePending>
  emittedAttached: boolean
  emittedCreated: boolean
  generation: number
  identity: ElectronGuestCdpIdentity
  outwardRootSessionId: string
  outwardTargetId: string
  surfaceId: string
  targetInfo: GroupTargetInfo
  transport: ElectronGuestCdpTransport
}

export type ManagedTargetCreationIntent = 'background' | 'interactive'

export interface ElectronSurfaceGroupCdpSurface {
  generation: number
  surfaceId: string
  transport: ElectronGuestCdpTransport
}

interface ElectronSurfaceGroupCreatedSurface {
  generation: number
  surfaceId: string
  /** Omitted when the Host admission scheduler already added this exact surface to the group. */
  transport?: ElectronGuestCdpTransport
}

export interface ElectronSurfaceGroupCdpTransportOptions {
  activateSurface: (input: { generation: number; surfaceId: string }) => Promise<void>
  closeSurface: (input: { generation: number; surfaceId: string }) => Promise<void>
  createSurface: (input: {
    activate: boolean
    intent: ManagedTargetCreationIntent
    purpose: 'context-control' | 'target'
    url: string
  }) => Promise<ElectronSurfaceGroupCreatedSurface>
  dispatchContextCommand?: (method: string, params: CdpParams) => Promise<unknown>
  forceRetireSurface?: (input: { generation: number; surfaceId: string }) => void
  getActiveSurfaceId: () => string | undefined
  handleSurfaceTransportClosed?: (input: { generation: number; surfaceId: string }) => void
}

const MAX_METHOD_LENGTH = 256
const MAX_SESSION_ID_LENGTH = 512
const MAX_GROUP_REQUEST_BYTES = 4 * 1_024 * 1_024
const MAX_GROUP_SURFACES = 16
const CONTEXT_CONTROL_TIMEOUT_MS = 10_000

/**
 * A private CDP compositor for one Host-owned BrowserSurfaceGroup.
 *
 * Each Electron webview keeps its proven, narrowly admitted ElectronGuestCdpTransport. This class
 * namespaces every target/session from those independent debugger clients and presents Playwright
 * with one synthetic persistent BrowserContext. It never discovers Electron targets: membership
 * is possible only through addSurface with a transport already admitted by BrowserTargetBroker.
 */
export class ElectronSurfaceGroupCdpTransport implements ConnectOverCDPTransport {
  private readonly browserContextId = `mycopilot-surface-group-${randomUUID()}`
  private readonly browserTargetId = `mycopilot-surface-group-browser-${randomUUID()}`
  private readonly contextControls = new Set<ContextControlPending>()
  private readonly entriesBySurface = new Map<string, SurfaceEntry>()
  private readonly entriesByOutwardSession = new Map<string, SurfaceEntry>()
  private readonly entriesByOutwardTarget = new Map<string, SurfaceEntry>()
  private readonly options: ElectronSurfaceGroupCdpTransportOptions
  private autoAttachParams?: CdpParams
  private closeHandler?: (reason?: string) => void
  private closeDelivered = false
  private closed = false
  private creationIntent: ManagedTargetCreationIntent = 'interactive'
  private creationIntentLeaseActive = false
  private discoverTargets = false
  private messageHandler?: (message: object) => void
  private nextDelegateId = 1

  constructor(options: ElectronSurfaceGroupCdpTransportOptions) {
    this.options = options
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
    this.deliverClose()
  }

  open(): void {
    // BrowserTargetBroker attaches every admitted delegate before it is added here.
  }

  send(message: object): void {
    if (this.closed) return
    const request = parseGroupRequest(message)
    if (!request) {
      this.closeWithReason('invalid_cdp_request')
      return
    }
    void this.dispatch(request)
  }

  close(): void {
    this.closeWithReason('client_closed')
  }

  /** Adds exactly one already-admitted surface generation. */
  async addSurface(input: ElectronSurfaceGroupCdpSurface): Promise<void> {
    this.assertOpen()
    if (
      this.entriesBySurface.size >= MAX_GROUP_SURFACES ||
      this.entriesBySurface.has(input.surfaceId)
    ) {
      throw new GroupCdpPolicyError('surface_conflict')
    }
    const identity = input.transport.managedIdentity()
    if (this.entriesByOutwardTarget.has(identity.targetId)) {
      throw new GroupCdpPolicyError('surface_conflict')
    }
    const entry: SurfaceEntry = {
      childOutwardSessionByRaw: new Map(),
      childOutwardTargetByRaw: new Map(),
      childParentRawSessionByRaw: new Map(),
      childRawSessionByOutward: new Map(),
      childRawTargetBySession: new Map(),
      childTargetInfoByRawSession: new Map(),
      delegatePending: new Map(),
      emittedAttached: false,
      emittedCreated: false,
      generation: input.generation,
      identity,
      outwardRootSessionId: `mycopilot-group-page-${randomUUID()}`,
      // Chromium uses a page targetId as that page's main-frame identity. Keep the exact admitted
      // Main-only ID so Page.getFrameTree/frame events continue to join CRPage's session map. The
      // group still controls enumeration and never projects this value to Renderer or Tool output.
      outwardTargetId: identity.targetId,
      surfaceId: input.surfaceId,
      targetInfo: {
        attached: true,
        browserContextId: this.browserContextId,
        canAccessOpener: false,
        targetId: identity.targetId,
        title: identity.targetInfo.title,
        type: 'page',
        url: identity.targetInfo.url
      },
      transport: input.transport
    }
    // Never expose a surface ID; target/session identities remain private to this transport.
    this.entriesBySurface.set(entry.surfaceId, entry)
    this.entriesByOutwardSession.set(entry.outwardRootSessionId, entry)
    this.entriesByOutwardTarget.set(entry.outwardTargetId, entry)
    input.transport.onmessage = (message) => this.handleDelegateMessage(entry, message)
    input.transport.onclose = (reason) => this.handleDelegateClosed(entry, reason)

    if (this.discoverTargets) this.emitTargetCreated(entry)
    if (this.autoAttachParams) {
      try {
        await this.callDelegate(entry, 'Target.setAutoAttach', this.autoAttachParams)
      } catch (error) {
        this.finalizeEntry(entry, true)
        input.transport.close()
        throw error
      }
    }
  }

  /** Removes one exact generation without disconnecting the remaining BrowserContext. */
  removeSurface(surfaceId: string, generation: number, closeTransport = true): void {
    const entry = this.entriesBySurface.get(surfaceId)
    if (!entry || entry.generation !== generation) return
    if (closeTransport) entry.transport.close()
    this.finalizeEntry(entry, true)
  }

  hasSurface(surfaceId: string, generation: number): boolean {
    const entry = this.entriesBySurface.get(surfaceId)
    return Boolean(entry && entry.generation === generation)
  }

  surfaceCount(): number {
    return this.entriesBySurface.size
  }

  /** One serialized upstream Tool call may select how its next Target.createTarget is surfaced. */
  setTargetCreationIntent(intent: ManagedTargetCreationIntent): () => void {
    this.assertOpen()
    if (this.creationIntentLeaseActive) {
      throw new GroupCdpPolicyError('target_creation_intent_busy')
    }
    this.creationIntentLeaseActive = true
    this.creationIntent = intent
    let finished = false
    return () => {
      if (finished) return
      finished = true
      this.creationIntent = 'interactive'
      this.creationIntentLeaseActive = false
    }
  }

  private async dispatch(request: CdpRequest): Promise<void> {
    try {
      const result = request.sessionId
        ? await this.dispatchSessionCommand(request)
        : await this.dispatchBrowserCommand(request)
      this.respond(request, { result: result ?? {} })
    } catch (error) {
      this.respond(request, {
        error: {
          code: -32_000,
          message:
            error instanceof GroupCdpPolicyError ? error.message : 'Managed group command failed'
        }
      })
    }
  }

  private async dispatchBrowserCommand(request: CdpRequest): Promise<unknown> {
    switch (request.method) {
      case 'Browser.getVersion': {
        const entry = this.selectedOrFirstEntry()
        if (!entry) return managedBrowserVersion()
        return await this.callDelegate(entry, request.method, request.params)
      }
      case 'Browser.setDownloadBehavior': {
        const entry = this.selectedOrFirstEntry()
        if (!entry) {
          this.assertGroupContext(request.params)
          return {}
        }
        return await this.callDelegate(
          entry,
          request.method,
          this.rewriteContextForDelegate(entry, request.params)
        )
      }
      case 'Storage.getCookies':
      case 'Storage.setCookies':
      case 'Storage.clearCookies':
        return await this.dispatchStorageCommand(request.method, request.params)
      case 'Target.setAutoAttach':
        return await this.setAutoAttach(request.params)
      case 'Target.setDiscoverTargets':
        return this.setDiscoverTargets(request.params)
      case 'Target.getTargets':
        return { targetInfos: this.targetInfos() }
      case 'Target.getTargetInfo':
        return { targetInfo: this.resolveTargetInfo(request.params) }
      case 'Target.createTarget':
        return await this.createTarget(request.params)
      case 'Target.closeTarget':
        return await this.closeTarget(request.params)
      case 'Target.activateTarget':
        return await this.activateTarget(request.params)
      case 'Target.detachFromTarget':
        return await this.detachChildTarget(request.params, undefined)
      default:
        throw new GroupCdpPolicyError('browser_command_not_permitted')
    }
  }

  private async dispatchSessionCommand(request: CdpRequest): Promise<unknown> {
    if (!request.sessionId) throw new GroupCdpPolicyError('target_session_not_admitted')
    const route = this.resolveSession(request.sessionId)
    if (request.method === 'Target.getTargets') return { targetInfos: this.targetInfos() }
    if (request.method === 'Target.getTargetInfo') {
      if (request.params?.targetId === undefined && route.rawSessionId) {
        const childInfo = route.entry.childTargetInfoByRawSession.get(route.rawSessionId)
        if (!childInfo) throw new GroupCdpPolicyError('target_not_admitted')
        return { targetInfo: { ...childInfo } }
      }
      return { targetInfo: this.resolveTargetInfo(request.params, route.entry) }
    }
    if (request.method === 'Target.detachFromTarget') {
      return await this.detachChildTarget(request.params, request.sessionId)
    }
    if (
      request.method === 'Target.createTarget' ||
      request.method === 'Target.closeTarget' ||
      request.method === 'Target.activateTarget'
    ) {
      return await this.dispatchBrowserCommand({ ...request, sessionId: undefined })
    }
    if (request.method.startsWith('Browser.') || request.method.startsWith('Storage.')) {
      throw new GroupCdpPolicyError('browser_command_requires_root')
    }
    if (request.method === 'Page.bringToFront' && route.rawSessionId === undefined) {
      await this.activateEntry(route.entry)
    }
    return await this.callDelegate(
      route.entry,
      request.method,
      request.params,
      route.rawSessionId ?? route.entry.identity.sessionId
    )
  }

  private async setAutoAttach(params: CdpParams | undefined): Promise<CdpParams> {
    if (params?.autoAttach !== true || params.flatten !== true) {
      throw new GroupCdpPolicyError('invalid_target_auto_attach')
    }
    this.autoAttachParams = { ...params, waitForDebuggerOnStart: false }
    // Insertion order is selected-first at initial connection; each delegate emits its admitted
    // attachedToTarget event synchronously before its command response.
    for (const entry of this.entriesBySurface.values()) {
      await this.callDelegate(entry, 'Target.setAutoAttach', this.autoAttachParams)
    }
    return {}
  }

  private setDiscoverTargets(params: CdpParams | undefined): CdpParams {
    if (!params || typeof params.discover !== 'boolean') {
      throw new GroupCdpPolicyError('invalid_target_discovery')
    }
    this.discoverTargets = params.discover
    if (this.discoverTargets) {
      for (const entry of this.entriesBySurface.values()) this.emitTargetCreated(entry)
    }
    return {}
  }

  private async createTarget(params: CdpParams | undefined): Promise<CdpParams> {
    const allowedKeys = new Set(['browserContextId', 'url'])
    if (
      !params ||
      Object.keys(params).some((key) => !allowedKeys.has(key)) ||
      typeof params.url !== 'string' ||
      !isManagedTargetUrl(params.url) ||
      (params.browserContextId !== undefined && params.browserContextId !== this.browserContextId)
    ) {
      throw new GroupCdpPolicyError('target_creation_not_permitted')
    }
    const intent = this.creationIntent
    const created = await this.options.createSurface({
      activate: intent === 'interactive',
      intent,
      purpose: 'target',
      url: params.url
    })
    try {
      if (!this.hasSurface(created.surfaceId, created.generation)) {
        if (!created.transport) throw new GroupCdpPolicyError('target_closed')
        await this.addSurface({ ...created, transport: created.transport })
      }
      const entry = this.entriesBySurface.get(created.surfaceId)
      if (!entry || entry.generation !== created.generation) {
        throw new GroupCdpPolicyError('target_closed')
      }
      // addSurface completes its auto-attach handshake before Target.createTarget returns, which
      // is required by CRBrowserContext.doCreateNewPage's synchronous _crPages lookup.
      if (intent === 'interactive') await this.activateEntry(entry)
      return { targetId: entry.outwardTargetId }
    } catch (error) {
      await this.options
        .closeSurface({ generation: created.generation, surfaceId: created.surfaceId })
        .catch(() => undefined)
      this.removeSurface(created.surfaceId, created.generation)
      throw error
    }
  }

  private async closeTarget(params: CdpParams | undefined): Promise<CdpParams> {
    const entry = this.resolveExactTarget(params)
    await this.options.closeSurface({ generation: entry.generation, surfaceId: entry.surfaceId })
    if (this.entriesBySurface.get(entry.surfaceId) === entry)
      this.removeSurface(entry.surfaceId, entry.generation)
    return { success: true }
  }

  private async activateTarget(params: CdpParams | undefined): Promise<CdpParams> {
    const entry = this.resolveExactTarget(params)
    await this.activateEntry(entry)
    return {}
  }

  private async detachChildTarget(
    params: CdpParams | undefined,
    callerOutwardSessionId: string | undefined
  ): Promise<unknown> {
    const outwardSessionId = params?.sessionId
    if (
      typeof outwardSessionId !== 'string' ||
      Object.keys(params ?? {}).some((key) => key !== 'sessionId')
    ) {
      throw new GroupCdpPolicyError('invalid_child_session')
    }
    const route = this.resolveSession(outwardSessionId)
    if (route.rawSessionId === undefined) {
      throw new GroupCdpPolicyError('root_target_detach_not_permitted')
    }
    const rawParentSession = route.entry.childParentRawSessionByRaw.get(route.rawSessionId)
    if (rawParentSession === undefined) throw new GroupCdpPolicyError('invalid_child_session')
    const expectedCaller = this.mapEnvelopeSession(route.entry, rawParentSession)
    if (!expectedCaller || callerOutwardSessionId !== expectedCaller) {
      throw new GroupCdpPolicyError('invalid_child_session')
    }
    return await this.callDelegate(
      route.entry,
      'Target.detachFromTarget',
      { sessionId: route.rawSessionId },
      rawParentSession || route.entry.identity.sessionId
    )
  }

  private async dispatchStorageCommand(
    method: string,
    params: CdpParams | undefined
  ): Promise<unknown> {
    this.assertGroupContext(params)
    const entry = this.selectedOrFirstEntry()
    if (entry) {
      return await this.callDelegate(entry, method, this.rewriteContextForDelegate(entry, params))
    }
    if (this.options.dispatchContextCommand) {
      return await this.options.dispatchContextCommand(method, withoutBrowserContextId(params))
    }

    // The persistent/default Playwright BrowserContext remains valid after the user closes its
    // last page. Electron exposes cookie CDP commands only through a page debugger, so create one
    // Host-owned background control surface for this command and remove it in the same lifecycle.
    const control = await this.options.createSurface({
      activate: false,
      intent: 'background',
      purpose: 'context-control',
      url: 'about:blank'
    })
    if (!control.transport) throw new GroupCdpPolicyError('target_closed')
    try {
      // This delegate is deliberately never added to entriesBySurface and never receives
      // Target.setAutoAttach. Fixed Playwright therefore keeps zero Pages while the Host uses the
      // exact isolated-partition guest only as a one-command Browser-domain control channel.
      return await this.callContextControlDelegate(
        control.transport,
        { generation: control.generation, surfaceId: control.surfaceId },
        method,
        withoutBrowserContextId(params)
      )
    } finally {
      try {
        await this.options.closeSurface({
          generation: control.generation,
          surfaceId: control.surfaceId
        })
      } catch {
        // Renderer reload/crash may lose the close acknowledgement. Retire Main's exact
        // generation synchronously so this invisible control cannot survive the Tool call.
        this.options.forceRetireSurface?.({
          generation: control.generation,
          surfaceId: control.surfaceId
        })
      }
      control.transport.close()
    }
  }

  private async callContextControlDelegate(
    transport: ElectronGuestCdpTransport,
    identity: { generation: number; surfaceId: string },
    method: string,
    params: CdpParams
  ): Promise<unknown> {
    const id = this.nextDelegateId++
    return await new Promise<unknown>((resolve, reject) => {
      let settled = false
      let timer: ReturnType<typeof setTimeout> | undefined
      let pending!: ContextControlPending
      const finish = (operation: () => void): void => {
        if (settled) return
        settled = true
        if (timer) clearTimeout(timer)
        this.contextControls.delete(pending)
        transport.onmessage = undefined
        transport.onclose = undefined
        operation()
      }
      pending = {
        cancel: (reason) =>
          finish(() => {
            transport.close()
            this.options.forceRetireSurface?.(identity)
            reject(new GroupCdpPolicyError(reason))
          }),
        transport
      }
      this.contextControls.add(pending)
      timer = setTimeout(
        () => pending.cancel('context_control_timeout'),
        CONTEXT_CONTROL_TIMEOUT_MS
      )
      transport.onclose = () =>
        finish(() => {
          this.options.forceRetireSurface?.(identity)
          reject(new GroupCdpPolicyError('target_closed'))
        })
      transport.onmessage = (message) => {
        if (!isRecord(message) || message.id !== id) return
        const error = isRecord(message.error) ? message.error : undefined
        if (error) {
          finish(() =>
            reject(
              new GroupCdpPolicyError(
                typeof error.message === 'string' ? error.message : 'managed_control_failed'
              )
            )
          )
          return
        }
        finish(() => resolve(message.result ?? {}))
      }
      try {
        transport.send({ id, method, params })
      } catch {
        pending.cancel('target_closed')
      }
    })
  }

  private async activateEntry(entry: SurfaceEntry): Promise<void> {
    await this.options.activateSurface({ generation: entry.generation, surfaceId: entry.surfaceId })
  }

  private selectedOrFirstEntry(): SurfaceEntry | undefined {
    const selected = this.options.getActiveSurfaceId()
    return (
      (selected ? this.entriesBySurface.get(selected) : undefined) ??
      this.entriesBySurface.values().next().value
    )
  }

  private targetInfos(): GroupTargetInfo[] {
    return [...this.entriesBySurface.values()].map((entry) => ({ ...entry.targetInfo }))
  }

  private resolveTargetInfo(params: CdpParams | undefined, fallback?: SurfaceEntry): CdpParams {
    const targetId = params?.targetId
    if (targetId === undefined) {
      const entry = fallback ?? this.selectedOrFirstEntry()
      if (!entry) {
        return {
          attached: true,
          canAccessOpener: false,
          targetId: this.browserTargetId,
          title: '',
          type: 'browser',
          url: ''
        }
      }
      return { ...entry.targetInfo }
    }
    if (typeof targetId !== 'string') throw new GroupCdpPolicyError('target_not_admitted')
    const entry = this.entriesByOutwardTarget.get(targetId)
    if (!entry) throw new GroupCdpPolicyError('target_not_admitted')
    return { ...entry.targetInfo }
  }

  private resolveExactTarget(params: CdpParams | undefined): SurfaceEntry {
    if (!params || !hasExactKeys(params, ['targetId']) || typeof params.targetId !== 'string') {
      throw new GroupCdpPolicyError('target_not_admitted')
    }
    const entry = this.entriesByOutwardTarget.get(params.targetId)
    if (!entry) throw new GroupCdpPolicyError('target_not_admitted')
    return entry
  }

  private resolveSession(outwardSessionId: string): {
    entry: SurfaceEntry
    rawSessionId?: string
  } {
    const entry = this.entriesByOutwardSession.get(outwardSessionId)
    if (!entry) throw new GroupCdpPolicyError('target_session_not_admitted')
    if (entry.outwardRootSessionId === outwardSessionId) return { entry }
    const rawSessionId = entry.childRawSessionByOutward.get(outwardSessionId)
    if (!rawSessionId) throw new GroupCdpPolicyError('target_session_not_admitted')
    return { entry, rawSessionId }
  }

  private assertGroupContext(params: CdpParams | undefined): void {
    if (
      params?.browserContextId !== undefined &&
      params.browserContextId !== this.browserContextId
    ) {
      throw new GroupCdpPolicyError('browser_context_not_admitted')
    }
  }

  private rewriteContextForDelegate(entry: SurfaceEntry, params: CdpParams | undefined): CdpParams {
    this.assertGroupContext(params)
    if (!params) return {}
    const rewritten = { ...params }
    if (rewritten.browserContextId !== undefined) {
      rewritten.browserContextId = entry.identity.browserContextId
    }
    return rewritten
  }

  private callDelegate(
    entry: SurfaceEntry,
    method: string,
    params?: CdpParams,
    sessionId?: string
  ): Promise<unknown> {
    this.assertOpen()
    if (this.entriesBySurface.get(entry.surfaceId) !== entry) {
      return Promise.reject(new GroupCdpPolicyError('target_closed'))
    }
    const id = this.nextDelegateId++
    return new Promise<unknown>((resolve, reject) => {
      entry.delegatePending.set(id, {
        reject,
        resolve: (reply) => {
          if (reply.error) {
            reject(
              new GroupCdpPolicyError(
                typeof reply.error.message === 'string'
                  ? reply.error.message
                  : 'managed_target_command_failed'
              )
            )
            return
          }
          resolve(reply.result ?? {})
        }
      })
      entry.transport.send({
        id,
        method,
        ...(params ? { params } : {}),
        ...(sessionId ? { sessionId } : {})
      })
    })
  }

  private handleDelegateMessage(entry: SurfaceEntry, message: object): void {
    if (this.closed || this.entriesBySurface.get(entry.surfaceId) !== entry || !isRecord(message)) {
      return
    }
    if (typeof message.id === 'number') {
      const pending = entry.delegatePending.get(message.id)
      if (!pending) return
      entry.delegatePending.delete(message.id)
      pending.resolve(message)
      return
    }
    if (typeof message.method !== 'string' || !isRecord(message.params)) return
    this.forwardDelegateEvent(entry, message.method, message.params, message.sessionId)
  }

  private forwardDelegateEvent(
    entry: SurfaceEntry,
    method: string,
    params: CdpParams,
    rawEnvelopeSession: unknown
  ): void {
    if (method === 'Target.attachedToTarget') {
      const rawSessionId = params.sessionId
      const rawTargetInfo = params.targetInfo
      if (typeof rawSessionId !== 'string' || !isRecord(rawTargetInfo)) return
      const isRoot =
        rawSessionId === entry.identity.sessionId &&
        rawTargetInfo.targetId === entry.identity.targetId
      const rawParentSession = isRoot
        ? undefined
        : this.normalizeChildParentSession(entry, rawEnvelopeSession)
      const outwardParentSession = isRoot
        ? undefined
        : rawParentSession
          ? this.mapEnvelopeSession(entry, rawParentSession)
          : undefined
      let outwardSessionId: string | undefined
      if (isRoot) {
        outwardSessionId = entry.outwardRootSessionId
      } else {
        if (!rawParentSession || !outwardParentSession) return
        outwardSessionId = this.admitChildSession(
          entry,
          rawSessionId,
          rawTargetInfo,
          rawParentSession
        )
      }
      if (!outwardSessionId) return
      if (isRoot) entry.emittedAttached = true
      this.emit({
        method,
        params: {
          ...params,
          sessionId: outwardSessionId,
          targetInfo: isRoot
            ? { ...entry.targetInfo }
            : this.rewriteChildTargetInfo(entry, rawTargetInfo)
        },
        ...(isRoot ? {} : { sessionId: outwardParentSession })
      })
      return
    }
    if (method === 'Target.detachedFromTarget') {
      const rawSessionId = params.sessionId
      if (typeof rawSessionId !== 'string') return
      const isRoot = rawSessionId === entry.identity.sessionId
      const outwardSessionId = isRoot
        ? entry.outwardRootSessionId
        : entry.childOutwardSessionByRaw.get(rawSessionId)
      if (!outwardSessionId) return
      const rawTargetId = params.targetId
      const outwardTargetId = isRoot
        ? entry.outwardTargetId
        : entry.childRawTargetBySession.get(rawSessionId)
      if (!isRoot && typeof rawTargetId === 'string' && rawTargetId !== outwardTargetId) return
      const rawParentSession = isRoot
        ? undefined
        : entry.childParentRawSessionByRaw.get(rawSessionId)
      const eventParentSession = isRoot
        ? undefined
        : this.normalizeChildParentSession(entry, rawEnvelopeSession)
      const outwardParentSession = rawParentSession
        ? this.mapEnvelopeSession(entry, rawParentSession)
        : undefined
      if (
        !isRoot &&
        (!rawParentSession ||
          !eventParentSession ||
          eventParentSession !== rawParentSession ||
          !outwardParentSession)
      ) {
        return
      }
      if (!isRoot) this.retireDescendants(entry, rawSessionId)
      this.emit({
        method,
        params: {
          ...params,
          sessionId: outwardSessionId,
          ...(outwardTargetId ? { targetId: outwardTargetId } : {})
        },
        ...(isRoot ? {} : { sessionId: outwardParentSession })
      })
      if (isRoot) entry.emittedAttached = false
      else this.forgetChildSession(entry, rawSessionId)
      return
    }

    // Discovery is synthesized from the Host-owned group. Never forward Chromium discovery
    // notifications that have not gone through BrowserTargetBroker admission.
    if (method === 'Target.targetCreated' || method === 'Target.targetDestroyed') return
    const browserRootEvent = method.startsWith('Browser.') || method === 'Target.targetInfoChanged'
    const outwardEnvelope = browserRootEvent
      ? undefined
      : this.mapEnvelopeSession(entry, rawEnvelopeSession)
    if (!browserRootEvent && !outwardEnvelope) return
    if (
      method === 'Target.targetInfoChanged' &&
      isRecord(params.targetInfo) &&
      params.targetInfo.targetId !== entry.identity.targetId &&
      (typeof params.targetInfo.targetId !== 'string' ||
        !entry.childOutwardTargetByRaw.has(params.targetInfo.targetId))
    ) {
      return
    }
    if (
      method === 'Target.targetInfoChanged' &&
      isRecord(params.targetInfo) &&
      params.targetInfo.targetId === entry.identity.targetId
    ) {
      entry.targetInfo = {
        ...entry.targetInfo,
        title: boundedString(params.targetInfo.title),
        url: boundedString(params.targetInfo.url)
      }
    }
    if (
      method === 'Target.targetInfoChanged' &&
      isRecord(params.targetInfo) &&
      typeof params.targetInfo.targetId === 'string' &&
      params.targetInfo.targetId !== entry.identity.targetId
    ) {
      const changedTargetId = params.targetInfo.targetId
      const rawSessionId = [...entry.childRawTargetBySession].find(
        ([, rawTargetId]) => rawTargetId === changedTargetId
      )?.[0]
      if (rawSessionId) {
        entry.childTargetInfoByRawSession.set(
          rawSessionId,
          this.rewriteChildTargetInfo(entry, params.targetInfo)
        )
      }
    }
    const rewrittenParams =
      method === 'Target.targetInfoChanged' && isRecord(params.targetInfo)
        ? { ...params, targetInfo: this.rewriteEventTargetInfo(entry, params.targetInfo) }
        : params
    this.emit({
      method,
      params: rewrittenParams,
      ...(outwardEnvelope ? { sessionId: outwardEnvelope } : {})
    })
  }

  private admitChildSession(
    entry: SurfaceEntry,
    rawSessionId: string,
    rawTargetInfo: CdpParams,
    rawParentSession: string
  ): string | undefined {
    const existing = entry.childOutwardSessionByRaw.get(rawSessionId)
    if (existing) return existing
    const rawTargetId = rawTargetInfo.targetId
    if (typeof rawTargetId !== 'string') return undefined
    const outwardSessionId = `mycopilot-group-child-${randomUUID()}`
    // OOPIF targetId is also its FrameTree frameId. Keep that already-admitted raw ID so
    // Playwright can join Target.attachedToTarget with Page.frameAttached/frameNavigated. Only
    // session IDs need a per-debugger namespace at the group boundary.
    const outwardTargetId = rawTargetId
    entry.childOutwardSessionByRaw.set(rawSessionId, outwardSessionId)
    entry.childRawSessionByOutward.set(outwardSessionId, rawSessionId)
    entry.childOutwardTargetByRaw.set(rawTargetId, outwardTargetId)
    entry.childRawTargetBySession.set(rawSessionId, rawTargetId)
    entry.childParentRawSessionByRaw.set(rawSessionId, rawParentSession)
    entry.childTargetInfoByRawSession.set(
      rawSessionId,
      this.rewriteChildTargetInfo(entry, rawTargetInfo)
    )
    this.entriesByOutwardSession.set(outwardSessionId, entry)
    return outwardSessionId
  }

  private rewriteChildTargetInfo(entry: SurfaceEntry, raw: CdpParams): CdpParams {
    const rawTargetId = raw.targetId
    const outwardTargetId =
      typeof rawTargetId === 'string' ? entry.childOutwardTargetByRaw.get(rawTargetId) : undefined
    if (!outwardTargetId) throw new GroupCdpPolicyError('target_not_admitted')
    return { ...raw, browserContextId: this.browserContextId, targetId: outwardTargetId }
  }

  private rewriteEventTargetInfo(entry: SurfaceEntry, raw: CdpParams): CdpParams {
    if (raw.targetId === entry.identity.targetId) return { ...entry.targetInfo }
    return this.rewriteChildTargetInfo(entry, raw)
  }

  private mapEnvelopeSession(entry: SurfaceEntry, raw: unknown): string | undefined {
    if (raw === undefined || raw === '' || raw === entry.identity.sessionId) {
      return entry.outwardRootSessionId
    }
    return typeof raw === 'string' ? entry.childOutwardSessionByRaw.get(raw) : undefined
  }

  private normalizeChildParentSession(entry: SurfaceEntry, raw: unknown): string | undefined {
    if (raw === undefined || raw === '' || raw === entry.identity.sessionId) {
      return entry.identity.sessionId
    }
    return typeof raw === 'string' && entry.childOutwardSessionByRaw.has(raw) ? raw : undefined
  }

  private forgetChildSession(entry: SurfaceEntry, rawSessionId: string): void {
    const outwardSessionId = entry.childOutwardSessionByRaw.get(rawSessionId)
    if (!outwardSessionId) return
    entry.childOutwardSessionByRaw.delete(rawSessionId)
    entry.childRawSessionByOutward.delete(outwardSessionId)
    this.entriesByOutwardSession.delete(outwardSessionId)
    const rawTargetId = entry.childRawTargetBySession.get(rawSessionId)
    entry.childRawTargetBySession.delete(rawSessionId)
    entry.childParentRawSessionByRaw.delete(rawSessionId)
    entry.childTargetInfoByRawSession.delete(rawSessionId)
    if (rawTargetId) entry.childOutwardTargetByRaw.delete(rawTargetId)
  }

  private retireDescendants(entry: SurfaceEntry, rawParentSessionId: string): void {
    for (const [rawChildSessionId, parentRawSessionId] of [...entry.childParentRawSessionByRaw]) {
      if (parentRawSessionId !== rawParentSessionId) continue
      this.retireDescendants(entry, rawChildSessionId)
      const outwardChildSessionId = entry.childOutwardSessionByRaw.get(rawChildSessionId)
      const rawChildTargetId = entry.childRawTargetBySession.get(rawChildSessionId)
      if (outwardChildSessionId) {
        this.emit({
          method: 'Target.detachedFromTarget',
          params: {
            sessionId: outwardChildSessionId,
            ...(rawChildTargetId ? { targetId: rawChildTargetId } : {})
          },
          sessionId:
            entry.childOutwardSessionByRaw.get(rawParentSessionId) ?? entry.outwardRootSessionId
        })
      }
      this.forgetChildSession(entry, rawChildSessionId)
    }
  }

  private handleDelegateClosed(entry: SurfaceEntry, _reason?: string): void {
    if (this.entriesBySurface.get(entry.surfaceId) !== entry) return
    this.finalizeEntry(entry, true)
    this.options.handleSurfaceTransportClosed?.({
      generation: entry.generation,
      surfaceId: entry.surfaceId
    })
  }

  private finalizeEntry(entry: SurfaceEntry, emitLifecycle: boolean): void {
    if (this.entriesBySurface.get(entry.surfaceId) !== entry) return
    if (emitLifecycle) this.retireDescendants(entry, entry.identity.sessionId)
    if (emitLifecycle && entry.emittedAttached) {
      this.emit({
        method: 'Target.detachedFromTarget',
        params: { sessionId: entry.outwardRootSessionId, targetId: entry.outwardTargetId }
      })
    }
    if (emitLifecycle && this.discoverTargets && entry.emittedCreated) {
      this.emit({ method: 'Target.targetDestroyed', params: { targetId: entry.outwardTargetId } })
    }
    this.entriesBySurface.delete(entry.surfaceId)
    this.entriesByOutwardSession.delete(entry.outwardRootSessionId)
    this.entriesByOutwardTarget.delete(entry.outwardTargetId)
    for (const outwardSessionId of entry.childRawSessionByOutward.keys()) {
      this.entriesByOutwardSession.delete(outwardSessionId)
    }
    for (const pending of entry.delegatePending.values()) {
      pending.reject(new GroupCdpPolicyError('target_closed'))
    }
    entry.delegatePending.clear()
    entry.childOutwardSessionByRaw.clear()
    entry.childRawSessionByOutward.clear()
    entry.childOutwardTargetByRaw.clear()
    entry.childParentRawSessionByRaw.clear()
    entry.childRawTargetBySession.clear()
    entry.childTargetInfoByRawSession.clear()
  }

  private emitTargetCreated(entry: SurfaceEntry): void {
    if (entry.emittedCreated) return
    entry.emittedCreated = true
    this.emit({ method: 'Target.targetCreated', params: { targetInfo: { ...entry.targetInfo } } })
  }

  private respond(
    request: CdpRequest,
    payload: { error?: { code: number; message: string }; result?: unknown }
  ): void {
    this.emit({
      id: request.id,
      ...(request.sessionId ? { sessionId: request.sessionId } : {}),
      ...payload
    })
  }

  private emit(message: object): void {
    if (!this.closed) this.messageHandler?.(message)
  }

  private closeWithReason(reason: string): void {
    if (this.closed) return
    this.closed = true
    for (const control of [...this.contextControls]) control.cancel(reason)
    for (const entry of [...this.entriesBySurface.values()]) {
      this.finalizeEntry(entry, false)
      entry.transport.close()
    }
    this.closeReason = reason
    this.deliverClose()
  }

  private closeReason?: string

  private deliverClose(): void {
    if (this.closeDelivered || this.closeReason === undefined || !this.closeHandler) return
    this.closeDelivered = true
    this.closeHandler(this.closeReason)
  }

  private assertOpen(): void {
    if (this.closed) throw new GroupCdpPolicyError('target_closed')
  }
}

class GroupCdpPolicyError extends Error {}

function managedBrowserVersion(): CdpParams {
  const chromeVersion = process.versions.chrome ?? '0.0.0.0'
  return {
    jsVersion: process.versions.v8 ?? '',
    product: `Chrome/${chromeVersion}`,
    protocolVersion: '1.3',
    revision: 'managed-electron-surface-group',
    userAgent: `Mozilla/5.0 Chrome/${chromeVersion}`
  }
}

function parseGroupRequest(value: object): CdpRequest | null {
  if (!isRecord(value)) return null
  if (!Number.isSafeInteger(value.id) || Number(value.id) < 0) return null
  if (
    typeof value.method !== 'string' ||
    value.method.length === 0 ||
    value.method.length > MAX_METHOD_LENGTH
  ) {
    return null
  }
  if (value.params !== undefined && !isRecord(value.params)) return null
  if (
    value.sessionId !== undefined &&
    (typeof value.sessionId !== 'string' ||
      value.sessionId.length === 0 ||
      value.sessionId.length > MAX_SESSION_ID_LENGTH)
  ) {
    return null
  }
  try {
    if (Buffer.byteLength(JSON.stringify(value), 'utf8') > MAX_GROUP_REQUEST_BYTES) return null
  } catch {
    return null
  }
  return {
    id: Number(value.id),
    method: value.method,
    ...(isRecord(value.params) ? { params: value.params } : {}),
    ...(typeof value.sessionId === 'string' ? { sessionId: value.sessionId } : {})
  }
}

function withoutBrowserContextId(params: CdpParams | undefined): CdpParams {
  const result = { ...(params ?? {}) }
  delete result.browserContextId
  return result
}

function hasExactKeys(value: CdpParams, expected: readonly string[]): boolean {
  const actual = Object.keys(value).sort()
  const sorted = [...expected].sort()
  return actual.length === sorted.length && actual.every((key, index) => key === sorted[index])
}

function isManagedTargetUrl(value: string): boolean {
  if (value === 'about:blank') return true
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:'
  } catch {
    return false
  }
}

function boundedString(value: unknown): string {
  return typeof value === 'string' ? value.slice(0, 4_096) : ''
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
