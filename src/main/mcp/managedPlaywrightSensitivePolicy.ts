import { createHash } from 'node:crypto'
import type {
  BuiltinMcpToolRiskKind,
  ManagedPlaywrightAuthorizationContext,
  ManagedPlaywrightBuiltinToolGrantContext
} from '@mycopilot/protocol'

import type { ManagedPlaywrightToolManifestEntry } from './managedPlaywrightManifest'

export interface ManagedPlaywrightSensitiveToolPolicy {
  readonly mode: 'always' | 'dynamic'
  readonly scope: string
  readonly riskKinds: readonly BuiltinMcpToolRiskKind[]
}

const POLICY = Object.freeze({
  // The official cookie tools operate on the whole managed BrowserContext. They are intentionally
  // not described as current-origin scoped even though approval still binds the active origin as
  // an execution-time TOCTOU fence.
  browser_cookie_clear: sensitive('managed_browser_profile', ['cookie_write']),
  browser_cookie_delete: sensitive('managed_browser_profile', ['cookie_write']),
  browser_cookie_get: sensitive('managed_browser_profile', ['cookie_read']),
  browser_cookie_list: sensitive('managed_browser_profile', ['cookie_read']),
  browser_cookie_set: sensitive('managed_browser_profile', ['cookie_write']),
  browser_drop: dynamic('file_upload', ['file_read', 'file_upload']),
  browser_evaluate: sensitive('page_script_execution', ['page_script_execution']),
  browser_file_upload: dynamic('file_upload', ['file_read', 'file_upload']),
  browser_localstorage_clear: sensitive('local_storage_write', ['local_storage_write']),
  browser_localstorage_delete: sensitive('local_storage_write', ['local_storage_write']),
  browser_localstorage_get: sensitive('local_storage_read', ['local_storage_read']),
  browser_localstorage_list: sensitive('local_storage_read', ['local_storage_read']),
  browser_localstorage_set: sensitive('local_storage_write', ['local_storage_write']),
  browser_network_request: sensitive('network_sensitive_read', ['network_sensitive_read']),
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
  activeOrigin: string
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
  const approvalOrigin = expectApprovalOrigin(input.modelArguments.approval_origin)
  if (
    grant.argumentsDigest !== expectedArgumentsDigest ||
    grant.origin !== approvalOrigin ||
    approvalOrigin !== input.activeOrigin ||
    !sameStringArray(grant.riskKinds, policy.riskKinds)
  ) {
    throw new ManagedPlaywrightSensitiveGrantError(
      approvalOrigin !== input.activeOrigin ? 'origin_drifted' : 'drifted'
    )
  }
  const expectedScopeDigest = canonicalSha256({
    schemaVersion: 2,
    toolId: input.reviewed.rawName,
    argumentsDigest: expectedArgumentsDigest,
    origin: approvalOrigin,
    riskKinds: [...policy.riskKinds],
    scope: policy.scope,
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

function expectApprovalOrigin(value: unknown): string {
  if (typeof value !== 'string' || value.length < 1 || value.length > 2_048) {
    throw new ManagedPlaywrightSensitiveGrantError('drifted')
  }
  let parsed: URL
  try {
    parsed = new URL(value)
  } catch {
    throw new ManagedPlaywrightSensitiveGrantError('drifted')
  }
  if (
    !['http:', 'https:'].includes(parsed.protocol) ||
    parsed.origin !== value ||
    parsed.username !== '' ||
    parsed.password !== '' ||
    parsed.pathname !== '/' ||
    parsed.search !== '' ||
    parsed.hash !== ''
  ) {
    throw new ManagedPlaywrightSensitiveGrantError('drifted')
  }
  return value
}

function sameStringArray(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index])
}
