import type { WorkflowAnchor, WorkflowDefinition, WorkflowEndpoint } from '@mycopilot/protocol'
import { arrangeWorkflowNodes } from './workflowNodeLayout'
import { endpointNode, normalizeAnchor, type FlowEnd } from './workflowAnchors'
import { workflowNodeSize } from './workflowAuthoring'
import type { CanvasGraph } from './workflowCanvasGeometry'

type Side = WorkflowAnchor['side']
const ends = ['source', 'target'] as const
const field = (end: FlowEnd) => (end === 'source' ? 'sourceAnchor' : 'targetAnchor')
const endpointKey = (endpoint: WorkflowEndpoint, end: FlowEnd) =>
  endpoint.kind === 'node' ? `node:${endpoint.nodeId}` : `boundary:${end}`

function card(graph: CanvasGraph, endpoint: WorkflowEndpoint, end: FlowEnd) {
  const node = endpointNode(graph, endpoint, end)
  if (!node) return null
  const kind = 'kind' in node && typeof node.kind === 'string' ? node.kind : undefined
  const size = workflowNodeSize(kind ? { kind } : undefined)
  return {
    ...node,
    ...size,
    kind,
    cx: node.x + size.width / 2,
    cy: node.y + size.height / 2
  }
}

/** Prefer facing sides across the gap between cards, including vertical and reverse links. */
function facingSide(
  from: NonNullable<ReturnType<typeof card>>,
  to: NonNullable<ReturnType<typeof card>>
): Side {
  const dx = to.cx - from.cx,
    dy = to.cy - from.cy
  const gapX = Math.abs(dx) - (from.width + to.width) / 2
  const gapY = Math.abs(dy) - (from.height + to.height) / 2
  const horizontal = gapX >= 0 && (gapY < 0 || gapX >= gapY)
  if (horizontal || (gapX < 0 && gapY < 0 && Math.abs(dx) >= Math.abs(dy)))
    return dx >= 0 ? 'right' : 'left'
  return dy >= 0 ? 'bottom' : 'top'
}

/** Missing anchors are automatic; explicit anchors are user placements and remain untouched. */
export function resolveWorkflowAnchors<T extends CanvasGraph>(graph: T): T {
  type Port = {
    flowIndex: number
    end: FlowEnd
    side: Side
    offset: number
    fixed: boolean
    opposite: number
    length: number
    key: string
  }
  const groups = new Map<string, Port[]>()
  const flows = graph.flows.map((flow) => ({ ...flow }))
  graph.flows.forEach((flow, flowIndex) => {
    for (const end of ends) {
      const node = card(graph, flow[end], end)
      const otherEnd = end === 'source' ? 'target' : 'source'
      const other = card(graph, flow[otherEnd], otherEnd)
      if (!node || !other) continue
      const stored = flow[field(end)]
      const gate = node.kind === 'inputGate' || node.kind === 'outputGate'
      const tip = gate && (node.kind === 'inputGate') === (end === 'source')
      const anchor = stored
        ? normalizeAnchor(node.kind, end, stored)
        : gate
          ? normalizeAnchor(node.kind, end)
          : { side: facingSide(node, other), offset: 0.5 }
      flows[flowIndex][field(end)] = anchor
      if (tip) continue
      const vertical = anchor.side === 'left' || anchor.side === 'right'
      const key = `${endpointKey(flow[end], end)}:${anchor.side}`
      const group = groups.get(key) ?? []
      group.push({
        flowIndex,
        end,
        side: anchor.side,
        offset: anchor.offset,
        fixed: !!stored,
        opposite: vertical ? other.cy : other.cx,
        length: (vertical ? node.height : node.width) - (gate ? 4 : 0),
        key: `${flow.id}:${end}`
      })
      groups.set(key, group)
    }
  })
  for (const ports of groups.values()) {
    const automatic = ports
      .filter((p) => !p.fixed)
      .sort((a, b) => a.opposite - b.opposite || a.key.localeCompare(b.key))
    if (!automatic.length) continue
    const margin = Math.min(0.2, 7 / ports[0].length)
    const fixed = ports.filter((p) => p.fixed).map((p) => p.offset)
    // Start evenly spaced; if manual points occupy those slots, fill the largest free gaps.
    const slots: number[] = []
    const spacing = Math.min(10 / ports[0].length, (1 - 2 * margin) / Math.max(1, ports.length))
    for (let i = 0; i < automatic.length; i++) {
      const offset = margin + ((1 - 2 * margin) * (i + 1)) / (automatic.length + 1)
      if (fixed.every((p) => Math.abs(p - offset) >= spacing)) slots.push(offset)
    }
    while (slots.length < automatic.length) {
      const occupied = [
        margin,
        ...fixed.filter((p) => p > margin && p < 1 - margin),
        ...slots,
        1 - margin
      ].sort((a, b) => a - b)
      let start = occupied[0],
        gap = -1
      for (let i = 1; i < occupied.length; i++) {
        if (occupied[i] - occupied[i - 1] > gap) {
          start = occupied[i - 1]
          gap = occupied[i] - occupied[i - 1]
        }
      }
      slots.push(start + gap / 2)
    }
    slots.sort((a, b) => a - b)
    automatic.forEach((port, i) => {
      flows[port.flowIndex][field(port.end)] = { side: port.side, offset: slots[i] }
    })
  }
  return { ...graph, flows }
}

/** One undoable edit releases manual placements; automatic layout is deterministic on reopen. */
export function optimizeWorkflowLayout(graph: WorkflowDefinition): WorkflowDefinition {
  return {
    ...arrangeWorkflowNodes(graph),
    flows: graph.flows.map((flow) => {
      const next = { ...flow }
      delete next.sourceAnchor
      delete next.targetAnchor
      return next
    })
  }
}
