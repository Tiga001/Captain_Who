import { unwrapHostInvocation } from '@mycopilot/host-api'
import type { WorkflowRequest, WorkflowResponse } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'

export async function requestWorkflows(input: WorkflowRequest): Promise<WorkflowResponse> {
  return unwrapHostInvocation(await hostClient.agent.requestWorkflows(input))
}
