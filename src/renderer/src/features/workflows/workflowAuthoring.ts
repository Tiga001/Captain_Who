import type {
  AgentTemplate,
  WorkflowDefinition,
  WorkflowEndpoint,
  WorkflowFlow,
  WorkflowNode,
  WorkflowRule
} from '@mycopilot/protocol'
import { getDefaultWorkflowBoundaryPositions } from '@mycopilot/protocol'

export const NODE_WIDTH = 184
export const NODE_HEIGHT = 64
export const WORKFLOW_DRAG_TYPE = 'application/x-captain-workflow-template'

export function createWorkflow(): WorkflowDefinition {
  return {
    schemaVersion: 1,
    id: crypto.randomUUID(),
    name: '',
    description: '',
    background: '',
    nodes: [],
    flows: [],
    boundaryPositions: getDefaultWorkflowBoundaryPositions([]),
    viewport: { x: 0, y: 0, zoom: 1 }
  }
}

export function createWorkflowRule(): WorkflowRule {
  return { mode: 'all', min: 0, max: 0, required: [], groups: [] }
}

export function createWorkflowNode(
  name: string,
  x: number,
  y: number,
  template?: AgentTemplate,
  modelConfigId: string | null = null
): WorkflowNode {
  return {
    id: crypto.randomUUID(),
    name: template?.name.slice(0, 128) ?? name,
    templateId: template?.templateId ?? null,
    modelConfigId: template ? null : modelConfigId,
    receives: '',
    task: (template?.instructions ?? '').slice(0, 32768),
    delivers: '',
    inputRule: createWorkflowRule(),
    outputRule: createWorkflowRule(),
    x,
    y
  }
}

export function endpointValue(endpoint: WorkflowEndpoint): string {
  return endpoint.kind === 'boundary' ? '' : endpoint.nodeId
}

export function workflowEndpoint(value: string): WorkflowEndpoint {
  return value ? { kind: 'node', nodeId: value } : { kind: 'boundary' }
}

export function nodeFlows(
  graph: WorkflowDefinition,
  nodeId: string,
  direction: 'input' | 'output'
): WorkflowFlow[] {
  return graph.flows.filter((flow) => {
    const endpoint = direction === 'input' ? flow.target : flow.source
    return endpoint.kind === 'node' && endpoint.nodeId === nodeId
  })
}

/** Removing a connection also removes its now-invalid membership references. */
export function removeWorkflowFlows(
  graph: WorkflowDefinition,
  removed: Set<string>
): WorkflowDefinition {
  const clean = (rule: WorkflowRule): WorkflowRule => ({
    ...rule,
    required: rule.required.filter((id) => !removed.has(id)),
    groups: rule.groups
      .map((group) => ({ ...group, flowIds: group.flowIds.filter((id) => !removed.has(id)) }))
      .filter((group) => group.flowIds.length > 0)
  })
  return {
    ...graph,
    flows: graph.flows.filter((flow) => !removed.has(flow.id)),
    nodes: graph.nodes.map((node) => ({
      ...node,
      inputRule: clean(node.inputRule),
      outputRule: clean(node.outputRule)
    }))
  }
}
