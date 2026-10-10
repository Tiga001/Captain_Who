import { unwrapHostInvocation, type AuthState } from '@mycopilot/host-api'
import type {
  WorkflowRequest,
  WorkflowResponse,
  WorkflowTemplateLanguage
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { invalidateWorkflowPages, setWorkflowPageAuthScope } from './workflowPageCache'

let watchingAuth = false
let watchingRuntime = false
let recoveryAuthScope: string | null = null
let recoveryAuthRevision = -1
let recoveryEpoch = 0
const pendingRecoveryReads = new Map<
  string,
  { instanceId: string | null; promise: Promise<WorkflowResponse> }
>()
function acceptWorkflowAuthState(state: AuthState): void {
  if (state.revision < recoveryAuthRevision) return
  recoveryAuthRevision = state.revision
  const scope = `${state.status}:${state.profile?.userId ?? ''}`
  setWorkflowPageAuthScope(scope)
  if (scope !== recoveryAuthScope) {
    recoveryAuthScope = scope
    recoveryEpoch += 1
    pendingRecoveryReads.clear()
  }
}
function watchWorkflowAuth(): void {
  if (!watchingAuth && hostClient.auth?.onStateChanged) {
    watchingAuth = true
    // Keep the memory cache scoped even when its last sidebar tab has been closed.
    hostClient.auth.onStateChanged(acceptWorkflowAuthState)
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
    ![
      'list',
      'listInstances',
      'getInstance',
      'validate',
      'runtimeSnapshot',
      'exportTemplateMarkdown'
    ].includes(input.operation)
  ) {
    notifyWorkflowMutation()
  }
  return response
}

function notifyWorkflowMutation(): void {
  pendingRecoveryReads.clear()
  invalidateWorkflowPages()
  if (typeof window !== 'undefined') window.dispatchEvent(new Event('captain:workflows-changed'))
}

export async function importWorkflowTemplate(): Promise<WorkflowResponse | null> {
  watchWorkflowAuth()
  if (recoveryAuthScope === null && hostClient.auth?.getState) {
    // Use revisions to order this snapshot against events delivered while it is pending.
    acceptWorkflowAuthState(await hostClient.auth.getState())
  }
  const epoch = recoveryEpoch
  const response = unwrapHostInvocation(await hostClient.agent.importWorkflowTemplate())
  if (epoch !== recoveryEpoch) throw new Error('Organization account session changed')
  if (response) notifyWorkflowMutation()
  return response
}

export async function exportWorkflowTemplate(input: {
  id: string
  expectedRevision: number
  language: WorkflowTemplateLanguage
}): Promise<{ saved: boolean }> {
  return unwrapHostInvocation(await hostClient.agent.exportWorkflowTemplate(input))
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
