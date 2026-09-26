import { unwrapHostInvocation } from '@mycopilot/host-api'
import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export async function requestWorkflows(input: WorkflowRequest): Promise<WorkflowResponse> {
  const response = unwrapHostInvocation(await hostClient.agent.requestWorkflows(input))
  if (
    typeof window !== 'undefined' &&
    ![
      'list',
      'listInstances',
      'validate',
      'runtimeSnapshot',
      'completeUserInput',
      'discardFailedInput'
    ].includes(input.operation)
  ) {
    window.dispatchEvent(new Event('captain:workflows-changed'))
  }
  return response
}
