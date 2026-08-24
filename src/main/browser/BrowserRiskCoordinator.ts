import { randomUUID } from 'node:crypto'
import type { ManagedPlaywrightBuiltinToolGrantContext } from '@mycopilot/protocol'

import {
  BrowserNetworkPolicy,
  type BrowserDestinationIdentity,
  type BrowserDestinationAssessmentOptions,
  type BrowserHostBoundaryCode,
  type BrowserRiskKind
} from './BrowserNetworkPolicy'

const DEFAULT_APPROVAL_TIMEOUT_MS = 15 * 60 * 1_000
const MAX_PENDING_APPROVALS = 32
const MAX_OPERATION_PENDING_CHECKS = 32
const MAX_REASSESSMENTS = 3

export type BrowserRiskTrigger =
  'tool_argument' | 'main_frame' | 'redirect' | 'new_window' | 'subresource' | 'upload' | 'download'

export type BrowserRiskDispatchCertainty = 'definitely_not_dispatched' | 'possibly_dispatched'

export interface BrowserRiskAuthorizationContext {
  runId: string
  capabilityId: 'browser_automation'
  activationId: string
  manifestDigest: string
  policyRevision: number
  grantExpiresAtMs: number
  invocationId: string
  callId: string
  triggerToolName: string
  callReason: string
  builtinToolGrant?: ManagedPlaywrightBuiltinToolGrantContext
}

export interface BrowserRiskAuthorizationRequest {
  schemaVersion: 1
  requestId: string
  parentRequestId: string
  authorizationContext: BrowserRiskAuthorizationContext
  destination: BrowserDestinationIdentity
  riskKinds: readonly BrowserRiskKind[]
  trigger: BrowserRiskTrigger
  dispatchCertainty: BrowserRiskDispatchCertainty
  createdAt: number
  expiresAt: number
}

export type BrowserRiskAuthorizationDecision =
  | { decision: 'approved'; grantId: string }
  | { decision: 'rejected'; reason?: string }
  | {
      decision:
        'cancelled' | 'expired' | 'policy_denied' | 'unsupported_host_boundary' | 'outcome_unknown'
      reason?: string
    }

export interface BrowserRiskAuthorizer {
  authorize(
    request: BrowserRiskAuthorizationRequest,
    signal: AbortSignal
  ): Promise<BrowserRiskAuthorizationDecision>
}

export type BrowserRiskFailureCode =
  | 'browser.risk_rejected'
  | 'browser.risk_cancelled'
  | 'browser.risk_expired'
  | 'browser.risk_policy_denied'
  | 'browser.risk_busy'
  | 'browser.risk_drift'
  | 'browser.risk_outcome_unknown'
  | 'browser.unsupported_host_boundary'

export interface BrowserRiskFailure {
  code: BrowserRiskFailureCode
  dispatchCertainty: BrowserRiskDispatchCertainty
  reason?: string
}

export class BrowserRiskError extends Error {
  readonly name = 'BrowserRiskError'

  constructor(readonly failure: BrowserRiskFailure) {
    super(failure.code)
  }
}

export interface BrowserRiskCheckInput {
  url: string
  trigger: BrowserRiskTrigger
  additionalRisks?: readonly BrowserRiskKind[]
  contextualRisks?: readonly BrowserRiskKind[]
  dispatchCertainty: BrowserRiskDispatchCertainty
}

export interface BrowserRiskOperationInput {
  authorizationContext: BrowserRiskAuthorizationContext
  parentRequestId: string
  signal?: AbortSignal
  /** Pauses only the enclosing Tool execution budget while a human decision is outstanding. */
  onApprovalWaitChange?: (waiting: boolean) => void
}

/**
 * Per-tool handle. A rejected webRequest is remembered so the MCP host can replace Chromium's
 * generic ERR_BLOCKED_BY_CLIENT with the typed Browser refusal outcome.
 */
export class BrowserRiskOperation {
  private failureValue?: BrowserRiskFailure
  private closed = false
  private readonly pending = new Set<Promise<unknown>>()

  constructor(
    private readonly coordinator: BrowserRiskCoordinator,
    readonly input: BrowserRiskOperationInput
  ) {}

  check(input: BrowserRiskCheckInput): Promise<void> {
    if (this.closed) {
      return Promise.reject(
        new BrowserRiskError({
          code: 'browser.risk_cancelled',
          dispatchCertainty: input.dispatchCertainty
        })
      )
    }
    if (this.pending.size >= MAX_OPERATION_PENDING_CHECKS) {
      const failure: BrowserRiskFailure = {
        code: 'browser.risk_busy',
        dispatchCertainty: input.dispatchCertainty
      }
      this.recordFailure(failure)
      return Promise.reject(new BrowserRiskError(failure))
    }
    return this.track(
      this.coordinator.check(this.input, input).catch((error: unknown) => {
        if (error instanceof BrowserRiskError) this.recordFailure(error.failure)
        throw error
      })
    )
  }

  /**
   * Tracks work caused by an already-authorized boundary event (for example loading an approved
   * popup URL or resuming the exact paused download). The MCP result must not settle before this
   * work has either completed or recorded a typed failure.
   */
  track<T>(operation: Promise<T>): Promise<T> {
    if (this.closed) return Promise.reject(new Error('browser.risk_operation_closed'))
    this.pending.add(operation)
    void operation.then(
      () => this.pending.delete(operation),
      () => this.pending.delete(operation)
    )
    return operation
  }

  async settle(): Promise<void> {
    while (this.pending.size > 0) {
      const settled = Promise.allSettled([...this.pending])
      if (this.input.signal) await raceWithSignal(settled, this.input.signal)
      else await settled
    }
  }

  recordFailure(failure: BrowserRiskFailure): void {
    const current = this.failureValue
    if (
      !current ||
      certaintyRank(failure.dispatchCertainty) > certaintyRank(current.dispatchCertainty)
    ) {
      this.failureValue = safeFailure(failure)
    }
  }

  failure(): BrowserRiskFailure | null {
    return this.failureValue ? safeFailure(this.failureValue) : null
  }

  close(): void {
    this.closed = true
  }
}

/**
 * Coordinates DNS revalidation and exact BrowserRiskApproval requests.
 *
 * It intentionally holds no persistent grant cache. Core owns task grants and decides whether an
 * identical request is already authorized. Main only deduplicates concurrent identical checks.
 */
export class BrowserRiskCoordinator {
  private readonly active = new Set<AbortController>()
  private readonly authorizer: BrowserRiskAuthorizer
  private readonly inFlight = new Map<string, Promise<BrowserRiskAuthorizationDecision>>()
  private readonly now: () => number
  private readonly policy: BrowserNetworkPolicy
  private readonly timeoutMs: number
  private disposed = false

  constructor(options: {
    authorizer: BrowserRiskAuthorizer
    now?: () => number
    policy: BrowserNetworkPolicy
    timeoutMs?: number
  }) {
    this.authorizer = options.authorizer
    this.now = options.now ?? Date.now
    this.policy = options.policy
    this.timeoutMs = normalizeTimeout(options.timeoutMs)
  }

  beginOperation(input: BrowserRiskOperationInput): BrowserRiskOperation {
    if (this.disposed) {
      throw new BrowserRiskError({
        code: 'browser.risk_cancelled',
        dispatchCertainty: 'definitely_not_dispatched'
      })
    }
    return new BrowserRiskOperation(this, input)
  }

  async check(operation: BrowserRiskOperationInput, input: BrowserRiskCheckInput): Promise<void> {
    if (this.disposed || operation.signal?.aborted) {
      throw riskError('browser.risk_cancelled', input.dispatchCertainty)
    }

    let previousIdentity: string | undefined
    for (let attempt = 0; attempt < MAX_REASSESSMENTS; attempt += 1) {
      const assessmentOptions = {
        additionalRisks: input.additionalRisks,
        contextualRisks: input.contextualRisks,
        signal: operation.signal
      }
      let assessment = await assessWithCancellation(
        this.policy,
        input.url,
        assessmentOptions,
        input.dispatchCertainty
      )
      if (assessment.disposition === 'deny') {
        throw hostBoundaryError(assessment.code, input.dispatchCertainty)
      }
      if (assessment.disposition === 'allow') {
        // A single public DNS answer is not an authorization boundary. Re-resolve in the managed
        // guest NetworkContext immediately before release so public-to-private rebinding becomes
        // an approval (or a hard Host-boundary denial), never an unchecked Chromium request.
        assessment = await assessWithCancellation(
          this.policy,
          input.url,
          assessmentOptions,
          input.dispatchCertainty
        )
        if (assessment.disposition === 'deny') {
          throw hostBoundaryError(assessment.code, input.dispatchCertainty)
        }
        if (assessment.disposition === 'allow') return
      }

      const identity = approvalIdentity(assessment.destination, assessment.riskKinds)
      if (previousIdentity === identity) {
        // The exact target was already approved and revalidated in the prior iteration.
        return
      }
      const decision = await this.authorize(operation, {
        destination: assessment.destination,
        riskKinds: assessment.riskKinds,
        trigger: input.trigger,
        dispatchCertainty: input.dispatchCertainty
      })
      assertApproved(decision, input.dispatchCertainty)

      // Never release a request solely on a stale DNS answer. Resolve and classify again. Only an
      // exact match may continue; drift creates a new approval on the next loop iteration.
      previousIdentity = identity
      const revalidated = await assessWithCancellation(
        this.policy,
        input.url,
        assessmentOptions,
        input.dispatchCertainty
      )
      if (revalidated.disposition === 'deny') {
        throw hostBoundaryError(revalidated.code, input.dispatchCertainty)
      }
      if (revalidated.disposition === 'allow') return
      if (approvalIdentity(revalidated.destination, revalidated.riskKinds) === identity) return
      previousIdentity = undefined
    }

    throw riskError('browser.risk_drift', input.dispatchCertainty)
  }

  async shutdown(): Promise<void> {
    if (this.disposed) return
    this.disposed = true
    for (const controller of this.active) controller.abort('shutdown')
    this.active.clear()
    await Promise.allSettled(this.inFlight.values())
    this.inFlight.clear()
  }

  snapshot(): { active: number; inFlight: number } {
    return { active: this.active.size, inFlight: this.inFlight.size }
  }

  private async authorize(
    operation: BrowserRiskOperationInput,
    input: Omit<
      BrowserRiskAuthorizationRequest,
      | 'schemaVersion'
      | 'requestId'
      | 'parentRequestId'
      | 'authorizationContext'
      | 'createdAt'
      | 'expiresAt'
    >
  ): Promise<BrowserRiskAuthorizationDecision> {
    const createdAt = this.now()
    const expiresAt = Math.min(
      createdAt + this.timeoutMs,
      operation.authorizationContext.grantExpiresAtMs
    )
    if (expiresAt <= createdAt) throw riskError('browser.risk_expired', input.dispatchCertainty)

    const key = deduplicationKey(operation, input.destination, input.riskKinds, input.trigger)
    const existing = this.inFlight.get(key)
    if (existing) {
      operation.onApprovalWaitChange?.(true)
      try {
        return await existing
      } finally {
        operation.onApprovalWaitChange?.(false)
      }
    }
    if (this.inFlight.size >= MAX_PENDING_APPROVALS) {
      throw riskError('browser.risk_busy', input.dispatchCertainty)
    }

    const controller = new AbortController()
    const abortFromOperation = (): void => controller.abort(operation.signal?.reason)
    operation.signal?.addEventListener('abort', abortFromOperation, { once: true })
    this.active.add(controller)
    const request: BrowserRiskAuthorizationRequest = {
      schemaVersion: 1,
      requestId: randomUUID(),
      parentRequestId: operation.parentRequestId,
      authorizationContext: cloneAuthorizationContext(operation.authorizationContext),
      destination: cloneDestination(input.destination),
      riskKinds: [...input.riskKinds],
      trigger: input.trigger,
      dispatchCertainty: input.dispatchCertainty,
      createdAt,
      expiresAt
    }
    operation.onApprovalWaitChange?.(true)
    const pending = raceWithSignal(
      this.authorizer.authorize(request, controller.signal),
      controller.signal
    )
      .catch((error: unknown) => {
        if (controller.signal.aborted) {
          return { decision: 'cancelled' } as const
        }
        throw error
      })
      .finally(() => {
        operation.onApprovalWaitChange?.(false)
        operation.signal?.removeEventListener('abort', abortFromOperation)
        this.active.delete(controller)
        if (this.inFlight.get(key) === pending) this.inFlight.delete(key)
      })
    this.inFlight.set(key, pending)
    return pending
  }
}

function assertApproved(
  decision: BrowserRiskAuthorizationDecision,
  dispatchCertainty: BrowserRiskDispatchCertainty
): asserts decision is Extract<BrowserRiskAuthorizationDecision, { decision: 'approved' }> {
  switch (decision.decision) {
    case 'approved':
      return
    case 'rejected':
      throw new BrowserRiskError({
        code: 'browser.risk_rejected',
        dispatchCertainty,
        ...(decision.reason ? { reason: boundReason(decision.reason) } : {})
      })
    case 'cancelled':
      throw riskError('browser.risk_cancelled', dispatchCertainty)
    case 'expired':
      throw riskError('browser.risk_expired', dispatchCertainty)
    case 'policy_denied':
      throw riskError('browser.risk_policy_denied', dispatchCertainty)
    case 'unsupported_host_boundary':
      throw riskError('browser.unsupported_host_boundary', dispatchCertainty)
    case 'outcome_unknown':
      throw riskError('browser.risk_outcome_unknown', 'possibly_dispatched')
  }
}

function hostBoundaryError(
  _code: BrowserHostBoundaryCode,
  dispatchCertainty: BrowserRiskDispatchCertainty
): BrowserRiskError {
  // The detailed boundary is deliberately not forwarded to Renderer or model. All such targets
  // share one safe, non-approvable error code.
  return riskError('browser.unsupported_host_boundary', dispatchCertainty)
}

function riskError(
  code: BrowserRiskFailureCode,
  dispatchCertainty: BrowserRiskDispatchCertainty
): BrowserRiskError {
  return new BrowserRiskError({ code, dispatchCertainty })
}

async function assessWithCancellation(
  policy: BrowserNetworkPolicy,
  url: string,
  options: BrowserDestinationAssessmentOptions,
  dispatchCertainty: BrowserRiskDispatchCertainty
) {
  try {
    return await policy.assess(url, options)
  } catch {
    if (options.signal?.aborted) {
      throw riskError('browser.risk_cancelled', dispatchCertainty)
    }
    throw riskError('browser.unsupported_host_boundary', dispatchCertainty)
  }
}

function approvalIdentity(
  destination: BrowserDestinationIdentity,
  riskKinds: readonly BrowserRiskKind[]
): string {
  return [
    destination.targetDigest,
    destination.resolutionFingerprint,
    destination.addressClass,
    ...riskKinds
  ].join(':')
}

function deduplicationKey(
  operation: BrowserRiskOperationInput,
  destination: BrowserDestinationIdentity,
  riskKinds: readonly BrowserRiskKind[],
  trigger: BrowserRiskTrigger
): string {
  const exactScope = requiresExactActionScope(riskKinds)
    ? [destination.targetDigest, trigger, operation.authorizationContext.triggerToolName]
    : []
  return [
    operation.authorizationContext.runId,
    operation.authorizationContext.activationId,
    operation.authorizationContext.manifestDigest,
    operation.authorizationContext.policyRevision,
    destination.origin,
    destination.scheme,
    destination.host,
    destination.port,
    destination.addressClass,
    destination.resolutionFingerprint,
    ...exactScope,
    ...riskKinds
  ].join(':')
}

function requiresExactActionScope(riskKinds: readonly BrowserRiskKind[]): boolean {
  return riskKinds.some(
    (kind) =>
      kind === 'url_userinfo' ||
      kind === 'risk_escalation' ||
      kind === 'new_window' ||
      kind === 'file_upload' ||
      kind === 'file_download'
  )
}

function cloneAuthorizationContext(
  value: BrowserRiskAuthorizationContext
): BrowserRiskAuthorizationContext {
  return { ...value }
}

function cloneDestination(value: BrowserDestinationIdentity): BrowserDestinationIdentity {
  return { ...value }
}

function safeFailure(value: BrowserRiskFailure): BrowserRiskFailure {
  return {
    code: value.code,
    dispatchCertainty: value.dispatchCertainty,
    ...(value.reason ? { reason: boundReason(value.reason) } : {})
  }
}

function boundReason(value: string): string {
  const sanitized = [...value]
    .map((character) => {
      const code = character.charCodeAt(0)
      return code <= 0x1f || code === 0x7f ? ' ' : character
    })
    .join('')
    .trim()
  return sanitized.length <= 1_024 ? sanitized : `${sanitized.slice(0, 1_021)}...`
}

function certaintyRank(value: BrowserRiskDispatchCertainty): number {
  return value === 'possibly_dispatched' ? 1 : 0
}

function normalizeTimeout(value: number | undefined): number {
  return Number.isSafeInteger(value) && value !== undefined && value > 0
    ? Math.min(value, DEFAULT_APPROVAL_TIMEOUT_MS)
    : DEFAULT_APPROVAL_TIMEOUT_MS
}

async function raceWithSignal<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) throw new Error('Browser risk authorization cancelled')
  return await new Promise<T>((resolve, reject) => {
    const abort = (): void => reject(new Error('Browser risk authorization cancelled'))
    signal.addEventListener('abort', abort, { once: true })
    operation.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort))
  })
}
