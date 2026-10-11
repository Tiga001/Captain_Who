import { AsyncLocalStorage } from 'node:async_hooks'
import type { HostDiagnosticContext } from '@mycopilot/host-api'

const context = new AsyncLocalStorage<Readonly<HostDiagnosticContext> | undefined>()

function parseDiagnosticContext(value: unknown): Readonly<HostDiagnosticContext> | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const candidate = value as Record<string, unknown>
  // Never log arbitrary supplied strings, extra fields or malformed metadata. This context is
  // untrusted observability only: no resource identity, permissions or routing decisions use it.
  if (Object.keys(candidate).length !== 2) return undefined
  if (
    typeof candidate.rendererSessionNonce !== 'string' ||
    !/^[a-f0-9]{32}$/.test(candidate.rendererSessionNonce) ||
    !Number.isSafeInteger(candidate.actionSequence) ||
    (candidate.actionSequence as number) <= 0
  )
    return undefined
  return Object.freeze({
    rendererSessionNonce: candidate.rendererSessionNonce,
    actionSequence: candidate.actionSequence as number
  })
}

/** Preserves the handler's original return value, Promise and synchronous exception semantics. */
export function withRpcDiagnosticContext<T>(value: unknown, operation: () => T): T {
  if (process.env.CAPTAIN_PERFORMANCE_DIAGNOSTICS !== '1') return operation()
  // Even invalid/missing metadata establishes a fresh scope, so a nested invocation cannot
  // accidentally inherit another Renderer action. AsyncLocalStorage isolates overlapping awaits.
  let diagnostic: Readonly<HostDiagnosticContext> | undefined
  try {
    diagnostic = parseDiagnosticContext(value)
  } catch {
    // Do not let exotic/malformed diagnostic metadata reject a valid business invocation.
  }
  return context.run(diagnostic, operation)
}

export function currentRpcDiagnosticContext(): Readonly<HostDiagnosticContext> | undefined {
  return context.getStore()
}

/** Process lifecycle/background subscriptions must not inherit whichever action started Core. */
export function withoutRpcDiagnosticContext<T>(operation: () => T): T {
  return context.exit(operation)
}
