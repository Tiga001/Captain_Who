import { unwrapHostInvocation } from '@mycopilot/host-api'
import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { invalidateWorkflowPages, setWorkflowPageAuthScope } from './workflowPageCache'

let watchingAuth = false
let watchingRuntime = false
let recoveryAuthScope: string | null = null
let recoveryEpoch = 0
const pendingRecoveryReads = new Map<
  string,
  { instanceId: string | null; promise: Promise<WorkflowResponse> }
>()
function watchWorkflowAuth(): void {
  if (!watchingAuth && hostClient.auth?.onStateChanged) {
    watchingAuth = true
    // Keep the memory cache scoped even when its last sidebar tab has been closed.
    hostClient.auth.onStateChanged((state) => {
      const scope = `${state.status}:${state.profile?.userId ?? ''}`
      setWorkflowPageAuthScope(scope)
      if (scope !== recoveryAuthScope) {
        recoveryAuthScope = scope
        recoveryEpoch += 1
        pendingRecoveryReads.clear()
      }
    })
  }
  if (!watchingRuntime && hostClient.agent.onWorkflowRuntimeChanged) {
    watchingRuntime = true
    hostClient.agent.onWorkflowRuntimeChanged((snapshot) => {
      // A later recovery must not join a query that predates a committed notification.
      for (const [key, read] of pendingRecoveryReads) {
        if (read.instanceId === null || read.instanceId === snapshot.instanceId)
          pendingRecoveryReads.delete(key)
      }
    })
  }
}

async function performRequest(input: WorkflowRequest, epoch?: number): Promise<WorkflowResponse> {
  const response = unwrapHostInvocation(await hostClient.agent.requestWorkflows(input))
  if (epoch !== undefined && epoch !== recoveryEpoch)
    throw new Error('Organization account session changed')
  if (
    typeof window !== 'undefined' &&
    !['list', 'listInstances', 'getInstance', 'validate', 'runtimeSnapshot'].includes(
      input.operation
    )
  ) {
    pendingRecoveryReads.clear()
    invalidateWorkflowPages()
    window.dispatchEvent(new Event('captain:workflows-changed'))
  }
  return response
}

export function requestWorkflows(input: WorkflowRequest): Promise<WorkflowResponse> {
  watchWorkflowAuth()
  const key =
    input.operation === 'listInstances'
      ? 'instances'
      : input.operation === 'runtimeSnapshot'
        ? JSON.stringify([
            input.instanceId,
            input.afterSequence ?? null,
            input.summaryOnly ?? false
          ])
        : null
  if (key === null) return performRequest(input)
  const pending = pendingRecoveryReads.get(key)
  if (pending) return pending.promise
  // Only in-flight identical reads are shared. Every later focus/poll/scope entry still reads
  // durable state, including the full recovery window (never substituted with a delta cursor).
  const promise = performRequest(input, recoveryEpoch).finally(() => {
    if (pendingRecoveryReads.get(key)?.promise === promise) pendingRecoveryReads.delete(key)
  })
  if (pendingRecoveryReads.size < 128)
    pendingRecoveryReads.set(key, {
      instanceId: input.operation === 'runtimeSnapshot' ? input.instanceId : null,
      promise
    })
  return promise
}
