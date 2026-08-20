import { createHash } from 'node:crypto'
import type {
  BuiltinMcpToolRiskKind,
  ManagedPlaywrightAuthorizationContext,
  ManagedPlaywrightBuiltinToolGrantContext,
  ManagedPlaywrightSensitiveBindingScope
} from '@mycopilot/protocol'

import type { ManagedPlaywrightToolManifestEntry } from './managedPlaywrightManifest'

export interface ManagedPlaywrightSensitiveToolPolicy {
  readonly mode: 'always' | 'dynamic'
  readonly scope: string
  readonly riskKinds: readonly BuiltinMcpToolRiskKind[]
}

const POLICY = Object.freeze({
  // The official cookie tools operate on the whole managed BrowserContext. Their approval binds
  // the managed profile generation and exact arguments, not an arbitrary active page origin.
  browser_cookie_clear: sensitive('managed_browser_profile', ['cookie_write']),
  browser_cookie_delete: sensitive('managed_browser_profile', ['cookie_write']),
  browser_cookie_get: sensitive('managed_browser_profile', ['cookie_read']),
  browser_cookie_list: sensitive('managed_browser_profile', ['cookie_read']),
  browser_cookie_set: sensitive('managed_browser_profile', ['cookie_write']),
  // These operations may address child Frames, a pending chooser, or the official request ledger.
  // Their authority is the active Host-owned Surface binding, not the top-frame origin alone.
  browser_drop: dynamic('managed_surface', ['file_read', 'file_upload']),
  browser_evaluate: sensitive('managed_surface', ['page_script_execution']),
  browser_file_upload: dynamic('managed_surface', ['file_read', 'file_upload']),
  browser_localstorage_clear: sensitive('local_storage_write', ['local_storage_write']),
  browser_localstorage_delete: sensitive('local_storage_write', ['local_storage_write']),
  browser_localstorage_get: sensitive('local_storage_read', ['local_storage_read']),
  browser_localstorage_list: sensitive('local_storage_read', ['local_storage_read']),
  browser_localstorage_set: sensitive('local_storage_write', ['local_storage_write']),
  browser_network_request: sensitive('managed_surface', ['network_sensitive_read']),
  browser_sessionstorage_clear: sensitive('session_storage_write', ['session_storage_write']),
  browser_sessionstorage_delete: sensitive('session_storage_write', ['session_storage_write']),
  browser_sessionstorage_get: sensitive('session_storage_read', ['session_storage_read']),
  browser_sessionstorage_list: sensitive('session_storage_read', ['session_storage_read']),
  browser_sessionstorage_set: sensitive('session_storage_write', ['session_storage_write']),
  browser_set_storage_state: sensitive('managed_browser_profile', [
    'file_read',
    'cookie_write',
    'local_storage_write',
    'storage_state_import'
  ]),
  browser_storage_state: sensitive('managed_browser_profile', [
    'file_write',
    'cookie_read',
    'local_storage_read',
    'storage_state_export'
  ])
} satisfies Record<string, ManagedPlaywrightSensitiveToolPolicy>)

export class ManagedPlaywrightSensitiveGrantError extends Error {
  readonly name = 'ManagedPlaywrightSensitiveGrantError'

  constructor(readonly code: 'missing' | 'drifted' | 'expired' | 'origin_drifted' | 'reused') {
    super(`mcp.builtin_playwright.sensitive_grant_${code}`)
  }
}

export interface ManagedPlaywrightSensitiveGrantLease {
  readonly grant: ManagedPlaywrightBuiltinToolGrantContext
  markDispatched(): void
}

export function sensitivePolicyForTool(
  toolName: string
): ManagedPlaywrightSensitiveToolPolicy | undefined {
  return POLICY[toolName as keyof typeof POLICY]
}

/**
 * Main/Core agree on the authority that must be frozen before approval. Cookie mutation is
 * normally BrowserContext-wide, but the pinned official `browser_cookie_set` derives an omitted
 * domain from the current Tab. That argument shape is therefore page-bound even though the user
 * facing approval continues to describe the managed browser profile mutation honestly.
 */
export function sensitiveBindingScopeForInvocation(
  toolName: string,
  argumentsValue: Readonly<Record<string, unknown>>
): ManagedPlaywrightSensitiveBindingScope {
  if (toolName === 'browser_cookie_set') {
    return typeof argumentsValue.domain === 'string' && argumentsValue.domain.length > 0
      ? 'managed_browser_profile'
      : 'managed_surface'
  }
  return sensitivePolicyForTool(toolName)?.scope === 'managed_browser_profile'
    ? 'managed_browser_profile'
    : 'managed_surface'
}

export function sensitiveToolNeedsGrant(
  toolName: string,
  argumentsValue: Readonly<Record<string, unknown>>
): boolean {
  const policy = sensitivePolicyForTool(toolName)
  if (!policy) return false
  if (policy.mode === 'always') return true
  if (toolName === 'browser_drop') {
    return Array.isArray(argumentsValue.paths) && argumentsValue.paths.length > 0
  }
  if (toolName === 'browser_file_upload') {
    return Array.isArray(argumentsValue.paths) && argumentsValue.paths.length > 0
  }
  return true
}

export function validateSensitiveToolGrant(input: {
  reviewed: ManagedPlaywrightToolManifestEntry
  modelArguments: Readonly<Record<string, unknown>>
  authorizationContext: ManagedPlaywrightAuthorizationContext | undefined
  activeOrigin: string | null
  consumedGrantIds: Set<string>
  now?: number
}): ManagedPlaywrightSensitiveGrantLease | undefined {
  const policy = sensitivePolicyForTool(input.reviewed.rawName)
  if (!policy || !sensitiveToolNeedsGrant(input.reviewed.rawName, input.modelArguments)) {
    if (input.authorizationContext?.builtinToolGrant) {
      throw new ManagedPlaywrightSensitiveGrantError('drifted')
    }
    return undefined
  }
  const authorization = input.authorizationContext
  const grant = authorization?.builtinToolGrant
  if (!authorization || !grant) throw new ManagedPlaywrightSensitiveGrantError('missing')
  const now = input.now ?? Date.now()
  if (grant.expiresAtMs <= now || grant.expiresAtMs > authorization.grantExpiresAtMs) {
    throw new ManagedPlaywrightSensitiveGrantError('expired')
  }
  if (input.consumedGrantIds.has(grant.grantId)) {
    throw new ManagedPlaywrightSensitiveGrantError('reused')
  }
  const expectedArgumentsDigest = canonicalSha256(input.modelArguments)
  if (
    grant.argumentsDigest !== expectedArgumentsDigest ||
    !sameStringArray(grant.riskKinds, policy.riskKinds)
  ) {
    throw new ManagedPlaywrightSensitiveGrantError('drifted')
  }
  if (grant.origin !== input.activeOrigin) {
    throw new ManagedPlaywrightSensitiveGrantError('origin_drifted')
  }
  const expectedScopeDigest = canonicalSha256({
    schemaVersion: 2,
    toolId: input.reviewed.rawName,
    argumentsDigest: expectedArgumentsDigest,
    origin: grant.origin,
    riskKinds: [...policy.riskKinds],
    scope: sensitiveBindingScopeForInvocation(input.reviewed.rawName, input.modelArguments),
    targetBindingDigest: grant.targetBindingDigest
  })
  if (grant.resourceScopeDigest !== expectedScopeDigest) {
    throw new ManagedPlaywrightSensitiveGrantError('drifted')
  }
  let dispatched = false
  return {
    grant,
    markDispatched: () => {
      if (dispatched || input.consumedGrantIds.has(grant.grantId)) {
        throw new ManagedPlaywrightSensitiveGrantError('reused')
      }
      dispatched = true
      input.consumedGrantIds.add(grant.grantId)
    }
  }
}

export function canonicalSha256(value: unknown): string {
  const canonical = canonicalJson(value)
  return `sha256:${createHash('sha256').update(JSON.stringify(canonical)).digest('hex')}`
}

function sensitive(
  scope: string,
  riskKinds: readonly BuiltinMcpToolRiskKind[]
): ManagedPlaywrightSensitiveToolPolicy {
  return Object.freeze({ mode: 'always', scope, riskKinds: Object.freeze([...riskKinds]) })
}

function dynamic(
  scope: string,
  riskKinds: readonly BuiltinMcpToolRiskKind[]
): ManagedPlaywrightSensitiveToolPolicy {
  return Object.freeze({ mode: 'dynamic', scope, riskKinds: Object.freeze([...riskKinds]) })
}

function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalJson)
  if (value && typeof value === 'object') {
    const record = value as Record<string, unknown>
    return Object.fromEntries(
      Object.keys(record)
        .sort()
        .map((key) => [key, canonicalJson(record[key])])
    )
  }
  return value
}

function sameStringArray(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index])
}
