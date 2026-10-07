import { unwrapHostInvocation } from '@mycopilot/host-api'
import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { invalidateWorkflowPages, setWorkflowPageAuthScope } from './workflowPageCache'

let watchingAuth = false
function watchWorkflowAuth(): void {
  if (watchingAuth || !hostClient.auth?.onStateChanged) return
  watchingAuth = true
  // Keep the memory cache scoped even when its last sidebar tab has been closed.
  hostClient.auth.onStateChanged((state) => {
    setWorkflowPageAuthScope(`${state.status}:${state.profile?.userId ?? ''}`)
  })
}

export async function requestWorkflows(input: WorkflowRequest): Promise<WorkflowResponse> {
  watchWorkflowAuth()
  const response = unwrapHostInvocation(await hostClient.agent.requestWorkflows(input))
  if (
    typeof window !== 'undefined' &&
    !['list', 'listInstances', 'getInstance', 'validate', 'runtimeSnapshot'].includes(
      input.operation
    )
  ) {
    invalidateWorkflowPages()
    window.dispatchEvent(new Event('captain:workflows-changed'))
  }
  return response
}
