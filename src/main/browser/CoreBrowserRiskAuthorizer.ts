import {
  BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
  parseBrowserRiskAuthorizeInput,
  type BrowserRiskAuthorizeInput,
  type BrowserRiskAuthorizeOutput,
  type BrowserRiskCancelInput
} from '@mycopilot/protocol'

import type {
  BrowserRiskAuthorizationDecision,
  BrowserRiskAuthorizationRequest,
  BrowserRiskAuthorizer
} from './BrowserRiskCoordinator'

export interface BrowserRiskAuthorizationCore {
  authorizeBrowserRisk(input: BrowserRiskAuthorizeInput): Promise<BrowserRiskAuthorizeOutput>
  cancelBrowserRisk(input: BrowserRiskCancelInput): Promise<boolean>
}

/** Strict Host-only adapter. Exact target/DNS HMACs cross only this Main-to-Core boundary. */
export class CoreBrowserRiskAuthorizer implements BrowserRiskAuthorizer {
  constructor(private readonly core: BrowserRiskAuthorizationCore) {}

  async authorize(
    request: BrowserRiskAuthorizationRequest,
    signal: AbortSignal
  ): Promise<BrowserRiskAuthorizationDecision> {
    if (signal.aborted) return { decision: 'cancelled' }
    const input = parseBrowserRiskAuthorizeInput({
      schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
      requestId: request.requestId,
      parentRequestId: request.parentRequestId,
      authorizationContext: request.authorizationContext,
      destination: {
        normalizedUrl: request.destination.displayUrl,
        origin: request.destination.origin,
        scheme: request.destination.scheme,
        asciiHost: request.destination.host,
        effectivePort: request.destination.port,
        addressClass: request.destination.addressClass,
        resolutionFingerprint: request.destination.resolutionFingerprint,
        targetFingerprint: request.destination.targetDigest
      },
      riskKinds: [...request.riskKinds],
      trigger: request.trigger,
      dispatchCertainty: request.dispatchCertainty,
      createdAtMs: request.createdAt,
      expiresAtMs: request.expiresAt
    })
    const abort = (): void => {
      void this.core
        .cancelBrowserRisk({
          schemaVersion: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
          requestId: request.requestId
        })
        .catch(() => undefined)
    }
    signal.addEventListener('abort', abort, { once: true })
    let output: BrowserRiskAuthorizeOutput
    try {
      output = await this.core.authorizeBrowserRisk(input)
    } finally {
      signal.removeEventListener('abort', abort)
    }
    switch (output.decision) {
      case 'approved':
        return { decision: 'approved', grantId: output.grantId! }
      case 'rejected':
        return { decision: 'rejected', ...(output.reason ? { reason: output.reason } : {}) }
      case 'cancelled':
      case 'expired':
      case 'policy_denied':
      case 'unsupported_host_boundary':
      case 'outcome_unknown':
        return { decision: output.decision, ...(output.reason ? { reason: output.reason } : {}) }
    }
  }
}
