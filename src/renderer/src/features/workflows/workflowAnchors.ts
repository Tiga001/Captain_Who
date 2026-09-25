import type { WorkflowAnchor, WorkflowEndpoint } from '@mycopilot/protocol'
import { workflowNodeSize } from './workflowAuthoring'
import type { CanvasGraph, CanvasPoint } from './workflowCanvasGeometry'

export type FlowEnd = 'source' | 'target'
export function endpointNode(graph: CanvasGraph, endpoint: WorkflowEndpoint, end: FlowEnd) {
  return endpoint.kind === 'node'
    ? graph.nodes.find((node) => node.id === endpoint.nodeId)
    : graph.boundaryPositions[end === 'source' ? 'input' : 'output']
}
export function normalizeAnchor(
  kind: string | undefined,
  end: FlowEnd,
  anchor?: WorkflowAnchor
): WorkflowAnchor {
  const side = end === 'source' ? 'right' : 'left'
  if (kind === 'inputGate' || kind === 'outputGate') {
    const tip = (kind === 'inputGate') === (end === 'source')
    return { side, offset: tip ? 0.5 : Math.max(0, Math.min(1, anchor?.offset ?? 0.5)) }
  }
  return anchor ?? { side, offset: 0.5 }
}
export function anchorPoint(
  graph: CanvasGraph,
  endpoint: WorkflowEndpoint,
  end: FlowEnd,
  anchor?: WorkflowAnchor
) {
  const node = endpointNode(graph, endpoint, end)
  if (!node) return null
  const kind = 'kind' in node && typeof node.kind === 'string' ? node.kind : undefined
  const size = workflowNodeSize(kind ? { kind } : undefined)
  const resolved = normalizeAnchor(kind, end, anchor)
  const gate = kind === 'inputGate' || kind === 'outputGate'
  const inset = gate ? 2 : 0
  const x =
    resolved.side === 'left'
      ? inset
      : resolved.side === 'right'
        ? size.width - inset
        : size.width * resolved.offset
  const y =
    resolved.side === 'top'
      ? 0
      : resolved.side === 'bottom'
        ? size.height
        : inset + (size.height - 2 * inset) * resolved.offset
  return { x: node.x + x, y: node.y + y, side: resolved.side }
}
export function anchorAtPoint(
  graph: CanvasGraph,
  endpoint: WorkflowEndpoint,
  end: FlowEnd,
  point: CanvasPoint
): WorkflowAnchor {
  const node = endpointNode(graph, endpoint, end)
  if (!node) return { side: end === 'source' ? 'right' : 'left', offset: 0.5 }
  const kind = 'kind' in node && typeof node.kind === 'string' ? node.kind : undefined
  const size = workflowNodeSize(kind ? { kind } : undefined)
  const x = Math.max(0, Math.min(size.width, point.x - node.x))
  const y = Math.max(0, Math.min(size.height, point.y - node.y))
  if (kind === 'inputGate' || kind === 'outputGate')
    return normalizeAnchor(kind, end, { side: 'left', offset: (y - 2) / (size.height - 4) })
  const sides: Array<[WorkflowAnchor['side'], number, number]> = [
    ['left', x, y / size.height],
    ['right', size.width - x, y / size.height],
    ['top', y, x / size.width],
    ['bottom', size.height - y, x / size.width]
  ]
  sides.sort((a, b) => a[1] - b[1])
  // Avoid rounded corners so the handle always sits on the visible border.
  const [side, , offset] = sides[0]
  const margin = 7 / (side === 'left' || side === 'right' ? size.height : size.width)
  return { side, offset: Math.max(margin, Math.min(1 - margin, offset)) }
}
