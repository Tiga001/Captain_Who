import {
  MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
  type AgentEvent,
  type ManagedPlaywrightBridgeErrorCode,
  type ManagedPlaywrightCancelNotification,
  type ManagedPlaywrightCommandNotification,
  type ManagedPlaywrightCompletionInput,
  type ManagedPlaywrightCompletionOutcome,
  type ManagedPlaywrightDispatchCertainty,
  type ManagedPlaywrightDispatchPhaseInput,
  parseManagedPlaywrightCancelNotification,
  parseManagedPlaywrightCommandNotification
} from '@mycopilot/protocol'

import {
  ManagedPlaywrightMcpHost,
  ManagedPlaywrightMcpHostError,
  type ManagedPlaywrightMcpHostOptions
} from './ManagedPlaywrightMcpHost'
import {
  BrowserFileBrokerError,
  type BrowserFileBroker,
  type BrowserFileOwner
} from '../browser/BrowserFileBroker'
import { MANAGED_PLAYWRIGHT_SERVER_ID } from './managedPlaywrightManifest'
import { managedPlaywrightTool } from './managedPlaywrightManifest'
import {
  ManagedPlaywrightSensitiveTargetBindingBroker,
  ManagedPlaywrightSensitiveTargetBindingError
} from './ManagedPlaywrightSensitiveTargetBindingBroker'
import { sensitivePolicyForTool } from './managedPlaywrightSensitivePolicy'

const MAX_IN_FLIGHT = 8
const CORE_COMPLETION_SETTLE_MS = 2_000

export interface ManagedPlaywrightBridgeCore {
  acknowledgeManagedPlaywrightDispatchPhase(
    input: ManagedPlaywrightDispatchPhaseInput
  ): Promise<boolean>
  completeManagedPlaywright(input: ManagedPlaywrightCompletionInput): Promise<boolean>
  onManagedPlaywrightCancel(
    handler: (input: ManagedPlaywrightCancelNotification) => void
  ): () => void
  onManagedPlaywrightCommand(
    handler: (input: ManagedPlaywrightCommandNotification) => void
  ): () => void
  onAgentEvent?(handler: (event: AgentEvent) => void): () => void
}

export interface ManagedPlaywrightBridgeHostOptions {
  completionSettleMs?: number
  core: ManagedPlaywrightBridgeCore
  createHost: () => ManagedPlaywrightMcpHost
  fileBroker?: BrowserFileBroker
  sensitiveTargetBindings: ManagedPlaywrightSensitiveTargetBindingBroker
  now?: () => number
}

interface ActiveCommand {
  controller: AbortController
  timer: ReturnType<typeof setTimeout>
}

/**
 * Exact Main-side endpoint for the Core-owned managed MCP peer.
 *
 * This is deliberately not a general JSON-RPC proxy. It accepts the reviewed managed operations,
 * keeps a
 * bounded request registry, validates every notification before use, and reports one terminal
 * completion for every admitted request. Raw browser/CDP identities never cross this boundary.
 */
export class ManagedPlaywrightBridgeHost {
  private readonly active = new Map<string, ActiveCommand>()
  private readonly core: ManagedPlaywrightBridgeCore
  private readonly completionSettleMs: number
  private readonly createHost: () => ManagedPlaywrightMcpHost
  private readonly fileBroker?: BrowserFileBroker
  private readonly now: () => number
  private readonly sensitiveTargetBindings: ManagedPlaywrightSensitiveTargetBindingBroker
  private readonly unsubscribeCancel: () => void
  private readonly unsubscribeCommand: () => void
  private readonly unsubscribeAgentEvent: () => void

  private host?: ManagedPlaywrightMcpHost
  private closed = false

  constructor(options: ManagedPlaywrightBridgeHostOptions) {
    this.core = options.core
    this.completionSettleMs = normalizeCompletionSettleMs(options.completionSettleMs)
    this.createHost = options.createHost
    this.fileBroker = options.fileBroker
    this.sensitiveTargetBindings = options.sensitiveTargetBindings
    this.now = options.now ?? Date.now
    this.unsubscribeCommand = this.core.onManagedPlaywrightCommand((input) =>
      this.acceptCommand(input)
    )
    this.unsubscribeCancel = this.core.onManagedPlaywrightCancel((input) =>
      this.cancelCommand(input)
    )
    this.unsubscribeAgentEvent =
      this.core.onAgentEvent?.((event) => {
        if (
          event.type === 'done' &&
          (event.status === 'completed' ||
            event.status === 'failed' ||
            event.status === 'cancelled')
        ) {
          this.sensitiveTargetBindings.releaseRun(event.runId)
          void this.fileBroker?.releaseRun(event.runId)
          void this.host?.releaseRun(event.runId)
        }
      }) ?? (() => undefined)
  }

  async close(): Promise<void> {
    if (this.closed) return
    this.closed = true
    this.unsubscribeCommand()
    this.unsubscribeCancel()
    this.unsubscribeAgentEvent()
    for (const active of this.active.values()) {
      clearTimeout(active.timer)
      active.controller.abort('shutdown')
    }
    this.active.clear()
    const host = this.host
    this.host = undefined
    if (host) await host.close()
    this.sensitiveTargetBindings.shutdown()
  }

  private acceptCommand(untrusted: ManagedPlaywrightCommandNotification): void {
    let input: ManagedPlaywrightCommandNotification
    try {
      input = parseManagedPlaywrightCommandNotification(untrusted)
    } catch {
      return
    }
    if (input.serverId !== MANAGED_PLAYWRIGHT_SERVER_ID) return
    // A duplicate id belongs to the already-admitted operation. Sending a second terminal
    // completion would race and could settle the Rust one-shot with the wrong operation.
    if (this.active.has(input.requestId)) return
    if (this.closed || this.active.size >= MAX_IN_FLIGHT) {
      void this.complete(input.requestId, errorOutcome('busy', 'definitely_not_dispatched'))
      return
    }
    if (input.deadlineMs <= this.now()) {
      void this.complete(input.requestId, errorOutcome('timeout', 'definitely_not_dispatched'))
      return
    }

    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort('timeout'), input.deadlineMs - this.now())
    this.active.set(input.requestId, { controller, timer })
    void this.execute(input, controller)
  }

  private cancelCommand(untrusted: ManagedPlaywrightCancelNotification): void {
    let input: ManagedPlaywrightCancelNotification
    try {
      input = parseManagedPlaywrightCancelNotification(untrusted)
    } catch {
      return
    }
    this.active.get(input.requestId)?.controller.abort(input.reason)
  }

  private async execute(
    input: ManagedPlaywrightCommandNotification,
    controller: AbortController
  ): Promise<void> {
    let outcome: ManagedPlaywrightCompletionOutcome
    let preparedBindingRequestId: string | undefined
    let dispatchCertainty: ManagedPlaywrightDispatchCertainty = 'definitely_not_dispatched'
    try {
      const timeoutMs = Math.max(1, Math.min(300_000, input.deadlineMs - this.now()))
      switch (input.command.type) {
        case 'connect': {
          const host = this.currentHost()
          await raceWithSignal(host.connect(controller.signal), controller.signal)
          outcome = { type: 'connected', protocol: host.protocolSnapshot() }
          break
        }
        case 'list_tools': {
          if (input.command.cursor !== null) {
            throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
          }
          const tools = await this.currentHost().listTools(controller.signal)
          outcome = {
            type: 'tools_listed',
            page: {
              tools: tools.map((tool) => ({
                name: tool.name,
                title: null,
                description: tool.description,
                inputSchema: tool.inputSchema,
                outputSchema: null,
                annotations: tool.annotations
              })),
              nextCursor: null,
              ttlMs: null,
              cacheScope: null
            }
          }
          break
        }
        case 'prepare_sensitive_tool': {
          const reviewed = managedPlaywrightTool(input.command.input.toolName)
          const sensitivePolicy = reviewed && sensitivePolicyForTool(reviewed.rawName)
          if (
            !reviewed ||
            reviewed.handlingMode !== 'approval_required' ||
            !sensitivePolicy ||
            (reviewed.rawName !== 'browser_cookie_set' &&
              input.command.input.bindingScope !==
                (sensitivePolicy.scope === 'managed_browser_profile'
                  ? 'managed_browser_profile'
                  : 'managed_surface'))
          ) {
            throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.tool_not_reviewed')
          }
          const fileOwner: BrowserFileOwner = {
            runId: input.command.input.runId,
            activationId: input.command.input.activationId,
            capabilityId: 'browser_automation',
            toolCallId: input.command.input.callId
          }
          let preparedFiles:
            | {
                handles: readonly string[]
                basenames: readonly string[]
                fileRevisionDigest: string
              }
            | undefined
          try {
            if (input.command.input.filePreparation) {
              if (!this.fileBroker) {
                throw new ManagedPlaywrightMcpHostError('browser.surface_unavailable')
              }
              const references = await raceWithSignal(
                this.fileBroker.freezeResolvedForRead({
                  owner: fileOwner,
                  paths: input.command.input.filePreparation.paths
                }),
                controller.signal
              )
              if (references.length === 0) {
                throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.invalid_arguments')
              }
              const resolution = await raceWithSignal(
                this.fileBroker.resolveForRead({
                  owner: fileOwner,
                  handles: references.map((reference) => reference.handle)
                }),
                controller.signal
              )
              preparedFiles = {
                handles: references.map((reference) => reference.handle),
                basenames: references.map((reference) => reference.displayName),
                fileRevisionDigest: resolution.fileRevisionDigest
              }
            }
            const prepared = this.sensitiveTargetBindings.prepare(
              input.command.input,
              preparedFiles
            )
            preparedBindingRequestId = input.command.input.bindingRequestId
            outcome = {
              type: 'sensitive_tool_prepared',
              ...prepared,
              fileBasenames: [...prepared.fileBasenames]
            }
          } catch (error) {
            if (input.command.input.filePreparation) {
              await this.fileBroker?.releaseToolCall(fileOwner)
            }
            throw error
          }
          break
        }
        case 'release_sensitive_tool_binding': {
          const released = this.sensitiveTargetBindings.release({
            bindingId: input.command.bindingId,
            runId: input.command.runId,
            activationId: input.command.activationId,
            callId: input.command.callId
          })
          await this.fileBroker?.releaseToolCall({
            runId: input.command.runId,
            toolCallId: input.command.callId
          })
          outcome = { type: 'sensitive_tool_binding_released', released }
          break
        }
        case 'call_tool': {
          const result = await this.currentHost().callTool(
            input.command.name,
            input.command.arguments,
            {
              signal: controller.signal,
              timeoutMs: Math.min(input.command.timeoutMs, timeoutMs),
              authorizationContext: input.command.authorizationContext,
              parentRequestId: input.requestId,
              onDispatchPhase: async (phase) => {
                const accepted = await this.core.acknowledgeManagedPlaywrightDispatchPhase({
                  schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                  requestId: input.requestId,
                  phase
                })
                if (accepted) {
                  dispatchCertainty =
                    phase === 'response_received' ? 'response_received' : 'possibly_dispatched'
                }
                return accepted
              }
            }
          )
          outcome = { type: 'tool_called', result }
          break
        }
        case 'close': {
          const host = this.host
          this.host = undefined
          if (host) await raceWithSignal(host.close(), controller.signal)
          outcome = { type: 'closed' }
          break
        }
      }
      if (controller.signal.aborted) throw cancellationError(controller.signal.reason)
    } catch (error) {
      if (input.command.type === 'connect') {
        // A timed-out/cancelled connect has no peer through which Core could later send `close`.
        // Retire this Host generation now; its bounded close invalidates any late official
        // connection so disable/shutdown cannot leave managed automation alive.
        const host = this.host
        this.host = undefined
        if (host) await host.close().catch(() => undefined)
      }
      outcome = mapError(error, input.command.type, dispatchCertainty)
    } finally {
      const active = this.active.get(input.requestId)
      if (active) clearTimeout(active.timer)
      this.active.delete(input.requestId)
    }
    const accepted = await this.complete(
      input.requestId,
      outcome,
      Boolean(preparedBindingRequestId)
    )
    if (preparedBindingRequestId && (!accepted || outcome.type !== 'sensitive_tool_prepared')) {
      this.sensitiveTargetBindings.releaseByRequestId(preparedBindingRequestId)
      if (input.command.type === 'prepare_sensitive_tool' && input.command.input.filePreparation) {
        await this.fileBroker?.releaseToolCall({
          runId: input.command.input.runId,
          toolCallId: input.command.input.callId
        })
      }
    }
  }

  private currentHost(): ManagedPlaywrightMcpHost {
    if (this.closed) throw new ManagedPlaywrightMcpHostError('mcp.builtin_playwright.closed')
    this.host ??= this.createHost()
    return this.host
  }

  private async complete(
    requestId: string,
    outcome: ManagedPlaywrightCompletionOutcome,
    boundSensitiveProposal = false
  ): Promise<boolean> {
    const completion = this.core
      .completeManagedPlaywright({
        schemaVersion: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
        requestId,
        outcome
      })
      .catch(() => false)
    if (!boundSensitiveProposal) return await completion
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        completion,
        new Promise<boolean>((resolve) => {
          timer = setTimeout(() => resolve(false), this.completionSettleMs)
          timer.unref?.()
        })
      ])
    } finally {
      if (timer) clearTimeout(timer)
    }
  }
}

function normalizeCompletionSettleMs(value: number | undefined): number {
  if (value === undefined) return CORE_COMPLETION_SETTLE_MS
  return Number.isSafeInteger(value) && value >= 10 && value <= CORE_COMPLETION_SETTLE_MS
    ? value
    : CORE_COMPLETION_SETTLE_MS
}

async function raceWithSignal<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) throw cancellationError(signal.reason)
  return new Promise<T>((resolve, reject) => {
    const abort = (): void => reject(cancellationError(signal.reason))
    signal.addEventListener('abort', abort, { once: true })
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}

function cancellationError(reason: unknown): ManagedPlaywrightMcpHostError {
  return new ManagedPlaywrightMcpHostError(
    reason === 'timeout' ? 'mcp.builtin_playwright.timeout' : 'mcp.builtin_playwright.cancelled'
  )
}

export function createManagedPlaywrightHostFactory(
  options: ManagedPlaywrightMcpHostOptions
): () => ManagedPlaywrightMcpHost {
  return () => new ManagedPlaywrightMcpHost(options)
}

function errorOutcome(
  code: ManagedPlaywrightBridgeErrorCode,
  dispatchCertainty: ManagedPlaywrightDispatchCertainty
): ManagedPlaywrightCompletionOutcome {
  return { type: 'error', code, dispatchCertainty }
}

function mapError(
  error: unknown,
  operation: ManagedPlaywrightCommandNotification['command']['type'],
  observedCertainty: ManagedPlaywrightDispatchCertainty = 'definitely_not_dispatched'
): ManagedPlaywrightCompletionOutcome {
  let certainty: ManagedPlaywrightDispatchCertainty = observedCertainty
  if (!(error instanceof ManagedPlaywrightMcpHostError)) {
    if (error instanceof ManagedPlaywrightSensitiveTargetBindingError) {
      const code: ManagedPlaywrightBridgeErrorCode =
        error.code === 'busy'
          ? 'busy'
          : error.code === 'surface_unavailable'
            ? 'surface_unavailable'
            : 'invalid_arguments'
      return errorOutcome(code, 'definitely_not_dispatched')
    }
    if (error instanceof BrowserFileBrokerError) {
      return errorOutcome(
        error.code === 'browser.file.capacity' ? 'busy' : 'invalid_arguments',
        'definitely_not_dispatched'
      )
    }
    return errorOutcome('internal_safe_error', certainty)
  }
  const codeByHostError: Record<string, ManagedPlaywrightBridgeErrorCode> = {
    'mcp.builtin_playwright.closed': 'closed',
    'mcp.builtin_playwright.busy': 'busy',
    'mcp.builtin_playwright.cancelled': 'cancelled',
    'mcp.builtin_playwright.timeout': 'timeout',
    'mcp.builtin_playwright.tool_not_reviewed': 'tool_not_reviewed',
    'mcp.builtin_playwright.invalid_arguments': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_grant_missing': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_grant_drifted': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_grant_expired': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_grant_origin_drifted': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_grant_reused': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_target_scope_unsupported': 'invalid_arguments',
    'mcp.builtin_playwright.sensitive_request_identity_unavailable': 'invalid_arguments',
    'mcp.builtin_playwright.catalog_drift': 'catalog_drift',
    'mcp.builtin_playwright.output_too_large': 'output_too_large',
    'mcp.builtin_playwright.protocol_error': 'protocol_error',
    'browser.surface_unavailable': 'surface_unavailable',
    'browser.surface_capacity_exceeded': 'surface_capacity_exceeded',
    'browser.target_closed': 'target_closed',
    'browser.risk_outcome_unknown': 'outcome_unknown'
  }
  const code = codeByHostError[error.code] ?? 'internal_safe_error'
  if (error.dispatchCertainty) certainty = error.dispatchCertainty
  else if (
    operation === 'call_tool' &&
    ['closed', 'busy', 'tool_not_reviewed', 'invalid_arguments', 'catalog_drift'].includes(code)
  ) {
    certainty = 'definitely_not_dispatched'
  } else if (operation === 'call_tool' && code === 'output_too_large') {
    certainty = 'response_received'
  }
  return errorOutcome(code, certainty)
}
