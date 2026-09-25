import type { WorkflowDefinition, WorkflowEndpoint, WorkflowFlow } from '@mycopilot/protocol'
import { NODE_HEIGHT, NODE_WIDTH } from './workflowAuthoring'
import { routeWorkflowFlow } from './workflowRouting'
import { addFlowCrossingBridges, roundedFlowPath } from './workflowEdgePaths'

export interface CanvasPoint {
  x: number
  y: number
}
export interface CanvasBounds {
  left: number
  top: number
  right: number
  bottom: number
}
export interface FlowBridge {
  point: CanvasPoint
  radius: number
  halfHeight: number
  direction: number
  segment: number
  start: CanvasPoint
  arc: string
  path: string
}
export interface FlowGeometry {
  cornerRadius: number
  bridges: FlowBridge[]
  start: CanvasPoint
  end: CanvasPoint
  path: string
  points: CanvasPoint[]
  label: CanvasPoint
}
const endpointKey = (endpoint: WorkflowEndpoint) =>
  endpoint.kind === 'boundary' ? 'boundary' : `node:${endpoint.nodeId}`

export type CanvasGraph = Pick<WorkflowDefinition, 'nodes' | 'flows' | 'boundaryPositions'>
type CanvasCard = CanvasPoint & { key: string }
function canvasCards(graph: CanvasGraph): CanvasCard[] {
  return [
    ...graph.nodes.map((node) => ({ key: `node:${node.id}`, x: node.x, y: node.y })),
    { key: 'boundary:input', ...graph.boundaryPositions.input },
    { key: 'boundary:output', ...graph.boundaryPositions.output }
  ]
}
function endpointCard(cards: CanvasCard[], endpoint: WorkflowEndpoint, side: 'input' | 'output') {
  return cards.find(
    (card) =>
      card.key === (endpoint.kind === 'boundary' ? `boundary:${side}` : endpointKey(endpoint))
  )
}

export function flowGeometry(graph: CanvasGraph, flow: WorkflowFlow): FlowGeometry | null {
  const cards = canvasCards(graph)
  const from = endpointCard(cards, flow.source, 'input')
  const to = endpointCard(cards, flow.target, 'output')
  if (!from || !to) return null
  const peers = graph.flows.filter(
    (candidate) =>
      endpointKey(candidate.source) === endpointKey(flow.source) &&
      endpointKey(candidate.target) === endpointKey(flow.target)
  )
  const lane = Math.max(
    0,
    peers.findIndex((candidate) => candidate.id === flow.id)
  )
  const start = { x: from.x + NODE_WIDTH, y: from.y + NODE_HEIGHT / 2 }
  const end = { x: to.x, y: to.y + NODE_HEIGHT / 2 }
  const outgoingEnds = graph.flows
    .filter((candidate) => endpointKey(candidate.source) === endpointKey(flow.source))
    .map((candidate) => endpointCard(cards, candidate.target, 'output'))
    .filter((card): card is CanvasCard => !!card && card.x > start.x)
  const incomingStarts = graph.flows
    .filter((candidate) => endpointKey(candidate.target) === endpointKey(flow.target))
    .map((candidate) => endpointCard(cards, candidate.source, 'input'))
    .filter((card): card is CanvasCard => !!card && card.x + NODE_WIDTH < end.x)
  const preferredX =
    outgoingEnds.length > 1
      ? (start.x + Math.min(...outgoingEnds.map((card) => card.x))) / 2
      : incomingStarts.length > 1
        ? (Math.max(...incomingStarts.map((card) => card.x + NODE_WIDTH)) + end.x) / 2
        : (start.x + end.x) / 2
  const { points, radius } = routeWorkflowFlow({
    start,
    end,
    from: from.key,
    to: to.key,
    preferredX,
    lane,
    laneCount: peers.length,
    cards: cards.map((card) => ({
      key: card.key,
      left: card.x,
      right: card.x + NODE_WIDTH,
      top: card.y,
      bottom: card.y + NODE_HEIGHT
    }))
  })
  // Put labels above the longest horizontal segment, away from bends and arrowheads.
  const horizontal = points
    .slice(1)
    .map((point, i) => ({ a: points[i], b: point }))
    .filter(({ a, b }) => a.y === b.y)
    .sort((a, b) => Math.abs(b.b.x - b.a.x) - Math.abs(a.b.x - a.a.x))[0]
  const label = horizontal
    ? { x: (horizontal.a.x + horizontal.b.x) / 2, y: horizontal.a.y - 12 }
    : { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 - 12 }
  return {
    start,
    end,
    points,
    cornerRadius: radius,
    bridges: [],
    path: roundedFlowPath(points, radius),
    label
  }
}

export function graphFlowGeometries(graph: CanvasGraph): Map<string, FlowGeometry | null> {
  return addFlowCrossingBridges(
    new Map(graph.flows.map((flow) => [flow.id, flowGeometry(graph, flow)])),
    canvasCards(graph).map((card) => ({
      left: card.x,
      right: card.x + NODE_WIDTH,
      top: card.y,
      bottom: card.y + NODE_HEIGHT
    }))
  )
}

export function graphBounds(
  graph: CanvasGraph,
  geometries = graphFlowGeometries(graph)
): CanvasBounds {
  const bounds: CanvasBounds[] = canvasCards(graph).map((node) => ({
    left: node.x - 9,
    top: node.y - 5,
    right: node.x + NODE_WIDTH + 9,
    bottom: node.y + NODE_HEIGHT + 5
  }))
  for (const flow of graph.flows) {
    const geometry = geometries.get(flow.id)
    if (!geometry) continue
    for (const point of geometry.points)
      bounds.push({ left: point.x - 5, top: point.y - 5, right: point.x + 5, bottom: point.y + 5 })
    for (const bridge of geometry.bridges)
      bounds.push({
        left: bridge.point.x - bridge.radius - 3,
        right: bridge.point.x + bridge.radius + 3,
        top: bridge.point.y - bridge.halfHeight - 3,
        bottom: bridge.point.y + bridge.halfHeight + 3
      })
    if (flow.name)
      bounds.push({
        left: geometry.label.x - 100,
        right: geometry.label.x + 100,
        top: geometry.label.y - 14,
        bottom: geometry.label.y + 6
      })
  }
  if (!bounds.length) return { left: 0, top: 0, right: 320, bottom: 160 }
  return {
    left: Math.min(...bounds.map((b) => b.left)),
    top: Math.min(...bounds.map((b) => b.top)),
    right: Math.max(...bounds.map((b) => b.right)),
    bottom: Math.max(...bounds.map((b) => b.bottom))
  }
}

export function fitViewport(
  bounds: CanvasBounds,
  width: number,
  height: number,
  origin: CanvasPoint
) {
  const contentWidth = Math.max(1, bounds.right - bounds.left)
  const contentHeight = Math.max(1, bounds.bottom - bounds.top)
  const zoom = Math.max(
    0.25,
    Math.min(1, Math.max(1, width - 96) / contentWidth, Math.max(1, height - 96) / contentHeight)
  )
  return {
    x: Math.min(
      100000,
      Math.max(0, (bounds.left + origin.x) * zoom - (width - contentWidth * zoom) / 2)
    ),
    y: Math.min(
      100000,
      Math.max(0, (bounds.top + origin.y) * zoom - (height - contentHeight * zoom) / 2)
    ),
    zoom
  }
}

export function canvasOrigin(bounds: CanvasBounds): CanvasPoint {
  return { x: Math.max(256, 72 - bounds.left), y: Math.max(160, 72 - bounds.top) }
}

export function zoomViewport(
  viewport: WorkflowDefinition['viewport'],
  zoom: number,
  anchor: CanvasPoint
) {
  const next = Math.min(2, Math.max(0.25, zoom))
  return {
    x: Math.min(100000, Math.max(0, ((viewport.x + anchor.x) * next) / viewport.zoom - anchor.x)),
    y: Math.min(100000, Math.max(0, ((viewport.y + anchor.y) * next) / viewport.zoom - anchor.y)),
    zoom: next
  }
}
