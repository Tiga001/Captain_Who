import type {
  WorkflowDefinition,
  WorkflowEndpoint,
  WorkflowFlow,
  WorkflowAgentNode,
  WorkflowUserNode,
  WorkflowGateNode,
  WorkflowNode,
  WorkflowRule
} from '@mycopilot/protocol'
import { getDefaultWorkflowBoundaryPositions } from '@mycopilot/protocol'

export const NODE_WIDTH = 184
export const NODE_HEIGHT = 52
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
    nextFlowSequence: 1,
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
  modelConfigId: string | null = null
): WorkflowAgentNode {
  return {
    id: crypto.randomUUID(),
    kind: 'agent',
    name: name.slice(0, 128),
    permissionMode: 'default',
    modelConfigId,
    receives: '',
    task: '',
    delivers: '',
    x,
    y
  }
}

export function createWorkflowUser(name: string, x: number, y: number): WorkflowUserNode {
  return { id: crypto.randomUUID(), kind: 'user', name: name.slice(0, 128), task: '', x, y }
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
  return {
    ...graph,
    flows: graph.flows.filter((flow) => !removed.has(flow.id)),
    nodes: graph.nodes.map((node) => {
      if (node.kind !== 'outputGate') return node
      const clean = <T extends { required: string[]; groups: { flowIds: string[] }[] }>(
        rule: T
      ): T => ({
        ...rule,
        required: rule.required.filter((id) => !removed.has(id)),
        // Keep an empty group: dropping it would silently relax a positive requirement.
        groups: rule.groups.map((group) => ({
          ...group,
          flowIds: group.flowIds.filter((id) => !removed.has(id))
        }))
      })
      return { ...node, selection: clean(node.selection) }
    })
  }
}

export function workflowNodeSize(node?: { kind: unknown }) {
  return node && (node.kind === 'inputGate' || node.kind === 'outputGate')
    ? { width: 64, height: 56, portY: 28 }
    : { width: NODE_WIDTH, height: NODE_HEIGHT, portY: NODE_HEIGHT / 2 }
}

export function createWorkflowGate(
  kind: WorkflowGateNode['kind'],
  x: number,
  y: number,
  nodes: readonly WorkflowNode[] = []
): WorkflowGateNode {
  const base = { id: crypto.randomUUID(), name: nextWorkflowGateName(kind, nodes), x, y }
  return kind === 'inputGate'
    ? {
        ...base,
        kind,
        processingMode: 'batch',
        busyPolicy: 'queue'
      }
    : { ...base, kind, selection: createWorkflowRule() }
}

function nextWorkflowGateName(
  kind: WorkflowGateNode['kind'],
  nodes: readonly WorkflowNode[]
): string {
  const prefix = kind === 'inputGate' ? '输入门' : '输出门'
  const sameKind = nodes.filter((node) => node.kind === kind)
  const numbers = sameKind.map((node) => {
    const suffix = node.name.startsWith(prefix) ? node.name.slice(prefix.length) : ''
    const number = /^\d+$/.test(suffix) ? Number(suffix) : 0
    return Number.isSafeInteger(number) ? number : 0
  })
  return `${prefix}${Math.max(sameKind.length, ...numbers, 0) + 1}`
}

export function nameUnnamedWorkflowGates(graph: WorkflowDefinition): WorkflowDefinition {
  const named = graph.nodes.filter((node) => node.name.trim())
  if (
    !graph.nodes.some(
      (node) => (node.kind === 'inputGate' || node.kind === 'outputGate') && !node.name.trim()
    )
  )
    return graph
  return {
    ...graph,
    nodes: graph.nodes.map((node) => {
      if ((node.kind !== 'inputGate' && node.kind !== 'outputGate') || node.name.trim()) return node
      const gate = { ...node, name: nextWorkflowGateName(node.kind, named) }
      named.push(gate)
      return gate
    })
  }
}

/** Reject self-connections and incompatible ports. Unconnected gates remain valid drafts. */
export function workflowConnectionAllowed(
  graph: WorkflowDefinition,
  source: WorkflowEndpoint,
  target: WorkflowEndpoint,
  excluding?: string
): boolean {
  const from = source.kind === 'node' ? graph.nodes.find((n) => n.id === source.nodeId) : null
  if (target.kind === 'boundary') return false
  const to = target.kind === 'node' ? graph.nodes.find((n) => n.id === target.nodeId) : null
  if ((source.kind === 'node' && !from) || (target.kind === 'node' && !to) || (!from && !to))
    return false
  if (from && to && from.id === to.id) return false
  if (from?.kind === 'inputGate') {
    if (to?.kind !== 'agent' && to?.kind !== 'user') return false
    if (
      graph.flows.some(
        (f) => f.id !== excluding && f.source.kind === 'node' && f.source.nodeId === from.id
      )
    )
      return false
    if (
      graph.flows.some(
        (f) =>
          f.id !== excluding &&
          f.target.kind === 'node' &&
          f.target.nodeId === to.id &&
          f.source.kind === 'node' &&
          graph.nodes.find((n) => n.id === (f.source as { nodeId: string }).nodeId)?.kind ===
            'inputGate'
      )
    )
      return false
  }
  if (to?.kind === 'outputGate') {
    if (from?.kind !== 'agent') return false
    if (
      graph.flows.some(
        (f) => f.id !== excluding && f.target.kind === 'node' && f.target.nodeId === to.id
      )
    )
      return false
    if (
      graph.flows.some(
        (f) =>
          f.id !== excluding &&
          f.source.kind === 'node' &&
          f.source.nodeId === from.id &&
          f.target.kind === 'node' &&
          graph.nodes.find((n) => n.id === (f.target as { nodeId: string }).nodeId)?.kind ===
            'outputGate'
      )
    )
      return false
  }
  return true
}

/** One history transaction: add/retarget a flow and insert any necessary gates.
 * External flow IDs survive rewiring, preserving rule membership and output labels.
 */
export function connectWorkflowFlow(
  graph: WorkflowDefinition,
  flow: WorkflowFlow
): WorkflowDefinition {
  if (!workflowConnectionAllowed(graph, flow.source, flow.target, flow.id)) return graph
  const old = graph.flows.find((f) => f.id === flow.id)
  let next: WorkflowDefinition = old
    ? { ...graph, flows: graph.flows.map((f) => (f.id === flow.id ? flow : f)) }
    : appendWorkflowFlow(graph, flow)
  for (const agent of graph.nodes.filter((n) => n.kind === 'agent' || n.kind === 'user')) {
    for (const direction of ['input', 'output'] as const) {
      if (agent.kind === 'user' && direction === 'output') continue
      const flows = nodeFlows(next, agent.id, direction)
      if (flows.length <= 1) continue
      const kind = direction === 'input' ? 'inputGate' : 'outputGate'
      const bindings = flows.filter((f) => {
        const ep = direction === 'input' ? f.source : f.target
        return ep.kind === 'node' && next.nodes.find((n) => n.id === ep.nodeId)?.kind === kind
      })
      if (bindings.length > 1) return graph
      const binding = bindings[0]
      const ep = binding && (direction === 'input' ? binding.source : binding.target)
      let gate = ep?.kind === 'node' ? next.nodes.find((n) => n.id === ep.nodeId) : undefined
      if (!gate) {
        let x = Math.max(
          -99000,
          Math.min(99000, agent.x + (direction === 'input' ? -120 : NODE_WIDTH + 56))
        )
        let y = Math.max(
          -99000,
          Math.min(99000, agent.y + NODE_HEIGHT / 2 - workflowNodeSize({ kind }).portY)
        )
        const occupied = [...next.nodes, next.boundaryPositions.input]
        while (
          occupied.some(
            (n) =>
              x < n.x + workflowNodeSize('kind' in n ? n : undefined).width + 20 &&
              x + 84 > n.x &&
              y < n.y + workflowNodeSize('kind' in n ? n : undefined).height + 20 &&
              y + 76 > n.y
          )
        ) {
          y += 80
          if (y > 99000) {
            y = -99000
            x += 224
          }
          if (x > 99000) return graph
        }
        gate = createWorkflowGate(kind, x, y, next.nodes)
        next = { ...next, nodes: [...next.nodes, gate] }
      }
      const gateEndpoint: WorkflowEndpoint = { kind: 'node', nodeId: gate.id }
      const ids = new Set(flows.filter((f) => f !== binding).map((f) => f.id))
      next = {
        ...next,
        flows: next.flows.map((f) =>
          ids.has(f.id)
            ? {
                ...f,
                [direction === 'input' ? 'target' : 'source']: gateEndpoint,
                [direction === 'input' ? 'targetAnchor' : 'sourceAnchor']: undefined
              }
            : f
        )
      }
      if (!binding)
        next = appendWorkflowFlow(next, {
          id: crypto.randomUUID(),
          name: '',
          source: direction === 'input' ? gateEndpoint : { kind: 'node', nodeId: agent.id },
          target: direction === 'input' ? { kind: 'node', nodeId: agent.id } : gateEndpoint,
          ...(direction === 'input'
            ? { targetAnchor: flows[0].targetAnchor }
            : { sourceAnchor: flows[0].sourceAnchor })
        })
    }
  }
  if (next.nodes.length > 128 || next.flows.length > 512) return graph
  // Retargeting removes membership only from gates that no longer own this flow.
  if (old)
    next = {
      ...next,
      nodes: next.nodes.map((node) => {
        if (node.kind !== 'outputGate') return node
        const endpoint = next.flows.find((f) => f.id === flow.id)!.source
        if (endpoint.kind === 'node' && endpoint.nodeId === node.id) return node
        return removeWorkflowFlows({ ...next, nodes: [node] }, new Set([flow.id])).nodes[0]
      })
    }
  return next
}

/** The saved high-water mark survives deletion, renaming, and reopening. */
export function nextWorkflowFlowSequence(graph: WorkflowDefinition): number {
  return Math.max(
    graph.nextFlowSequence ?? 1,
    ...graph.flows.map((flow) => {
      const match = /^S([1-9]\d*)$/.exec(flow.name)
      const value = match ? Number(match[1]) : 0
      return Number.isSafeInteger(value) ? value + 1 : 1
    })
  )
}

function appendWorkflowFlow(graph: WorkflowDefinition, flow: WorkflowFlow): WorkflowDefinition {
  const sequence = nextWorkflowFlowSequence(graph)
  return {
    ...graph,
    nextFlowSequence: sequence + 1,
    flows: [...graph.flows, { ...flow, name: flow.name || `S${sequence}` }]
  }
}

export function nameUnnamedWorkflowFlows(graph: WorkflowDefinition): WorkflowDefinition {
  let sequence = nextWorkflowFlowSequence(graph)
  if (!graph.flows.some((flow) => !flow.name.trim())) return graph
  return {
    ...graph,
    flows: graph.flows.map((flow) =>
      flow.name.trim() ? flow : { ...flow, name: `S${sequence++}` }
    ),
    nextFlowSequence: sequence
  }
}
