import type { WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
export function workflowNodeQueue(snapshot: WorkflowRuntimeSnapshot | null, nodeId: string) {
  const inputs = snapshot?.inputs.filter((input) => input.nodeId === nodeId) ?? []
  const waiting = inputs.filter((input) => input.mailStatus === 'pending')
  const processing = inputs.filter((input) => input.mailStatus === 'processing')
  return { inputs, waiting, processing, queuedMessages: waiting.flatMap((input) => input.messages) }
}
