import { unwrapHostInvocation } from '@mycopilot/host-api'
import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { invalidateWorkflowPages } from './workflowPageCache'

export async function requestWorkflows(input: WorkflowRequest): Promise<WorkflowResponse> {
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
