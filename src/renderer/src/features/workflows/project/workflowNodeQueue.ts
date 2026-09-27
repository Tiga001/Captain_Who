import type { WorkflowDefinition, WorkflowRuntimeSnapshot } from '@mycopilot/protocol'

export function workflowNodeQueue(snapshot: WorkflowRuntimeSnapshot | null, nodeId: string) {
  const inputs = snapshot?.inputs.filter((input) => input.nodeId === nodeId) ?? []
  const collecting =
    snapshot?.pendingMessages?.filter((message) => message.targetNodeId === nodeId) ?? []
  const waiting = inputs.filter((input) => ['pending', 'paused', 'failed'].includes(input.status))
  const queuedMessages = [...collecting, ...waiting.flatMap((input) => input.messages)]
  const runs = new Map(snapshot?.inputRuns?.map((run) => [run.inputId, run.status]))
  const processing = inputs.filter(
    (input) =>
      input.status === 'claimed' ||
      (input.status === 'applied' && runs.get(input.id) === 'in_progress')
  )
  return { inputs, collecting, waiting, queuedMessages, processing, runs }
}

export function inputGateRecipient(graph: WorkflowDefinition, gateId: string): string | null {
  const flow = graph.flows.find(
    (flow) => flow.source.kind === 'node' && flow.source.nodeId === gateId
  )
  return flow?.target.kind === 'node' ? flow.target.nodeId : null
}
