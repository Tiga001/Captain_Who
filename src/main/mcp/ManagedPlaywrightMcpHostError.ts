export type ManagedPlaywrightMcpHostErrorCode =
  | 'mcp.builtin_playwright.closed'
  | 'mcp.builtin_playwright.busy'
  | 'mcp.builtin_playwright.cancelled'
  | 'mcp.builtin_playwright.timeout'
  | 'mcp.builtin_playwright.queue_timeout'
  | 'mcp.builtin_playwright.tool_not_reviewed'
  | 'mcp.builtin_playwright.invalid_arguments'
  | 'mcp.builtin_playwright.sensitive_grant_missing'
  | 'mcp.builtin_playwright.sensitive_grant_drifted'
  | 'mcp.builtin_playwright.sensitive_grant_expired'
  | 'mcp.builtin_playwright.sensitive_grant_origin_drifted'
  | 'mcp.builtin_playwright.sensitive_grant_reused'
  | 'mcp.builtin_playwright.sensitive_target_scope_unsupported'
  | 'mcp.builtin_playwright.sensitive_request_identity_unavailable'
  | 'mcp.builtin_playwright.catalog_drift'
  | 'mcp.builtin_playwright.output_too_large'
  | 'mcp.builtin_playwright.protocol_error'
  | 'browser.surface_unavailable'
  | 'browser.surface_capacity_exceeded'
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
