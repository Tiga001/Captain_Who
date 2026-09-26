import { improveGateAnchorOrder } from './workflowGatePortOptimization'
import { resolveWorkflowAnchors } from './workflowAnchorLayout'
import { optimizeWorkflowRoutes } from './workflowRouteOptimization'
import type { WorkflowDefinition, WorkflowEndpoint, WorkflowFlow } from '@mycopilot/protocol'
import { workflowNodeSize } from './workflowAuthoring'
import { routeAnchoredFlow } from './workflowRouting'
import { anchorPoint } from './workflowAnchors'
import { addFlowCrossingBridges, roundedFlowPath, flowLabelPosition } from './workflowEdgePaths'

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
type CanvasCard = CanvasPoint & { key: string; width: number; height: number; portY: number }
function canvasCards(graph: CanvasGraph): CanvasCard[] {
  return [
    ...graph.nodes.map((node) => ({
      key: `node:${node.id}`,
      x: node.x,
      y: node.y,
      ...workflowNodeSize(node)
    })),
    { key: 'boundary:input', ...graph.boundaryPositions.input, ...workflowNodeSize() }
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
  const source = anchorPoint(graph, flow.source, 'source', flow.sourceAnchor)!
  const target = anchorPoint(graph, flow.target, 'target', flow.targetAnchor)!
  const start = { x: source.x, y: source.y }
  const end = { x: target.x, y: target.y }
  const outgoingEnds = graph.flows
    .filter((candidate) => endpointKey(candidate.source) === endpointKey(flow.source))
    .map((candidate) => endpointCard(cards, candidate.target, 'output'))
    .filter((card): card is CanvasCard => !!card && card.x > start.x)
  const incomingStarts = graph.flows
    .filter((candidate) => endpointKey(candidate.target) === endpointKey(flow.target))
    .map((candidate) => endpointCard(cards, candidate.source, 'input'))
    .filter((card): card is CanvasCard => !!card && card.x + card.width < end.x)
  const preferredX =
    outgoingEnds.length > 1
      ? (start.x + Math.min(...outgoingEnds.map((card) => card.x))) / 2
      : incomingStarts.length > 1
        ? (Math.max(...incomingStarts.map((card) => card.x + card.width)) + end.x) / 2
        : (start.x + end.x) / 2
  const { points, radius } = routeAnchoredFlow({
    sourceSide: source.side,
    targetSide: target.side,
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
      right: card.x + card.width,
      top: card.y,
      bottom: card.y + card.height
    }))
  })
  return {
    start,
    end,
    points,
    cornerRadius: radius,
    bridges: [],
    path: roundedFlowPath(points, radius),
    label: flowLabelPosition(points)
  }
}

export function graphFlowLayout(definition: CanvasGraph) {
  const resolved = resolveWorkflowAnchors(definition)
  const { graph, routes } = improveGateAnchorOrder(
    definition,
    resolved,
    new Map(resolved.flows.map((flow) => [flow.id, flowGeometry(resolved, flow)])),
    flowGeometry
  )
  const geometries = addFlowCrossingBridges(
    optimizeWorkflowRoutes(
      graph,
      routes,
      canvasCards(graph).map((card) => ({
        key: card.key,
        left: card.x,
        right: card.x + card.width,
        top: card.y,
        bottom: card.y + card.height
      }))
    ),
    canvasCards(graph).map((card) => ({
      left: card.x,
      right: card.x + card.width,
      top: card.y,
      bottom: card.y + card.height
    }))
  )
  return { graph, geometries }
}

export function graphFlowGeometries(graph: CanvasGraph): Map<string, FlowGeometry | null> {
  return graphFlowLayout(graph).geometries
}

export function graphBounds(
  graph: CanvasGraph,
  geometries = graphFlowGeometries(graph)
): CanvasBounds {
  const bounds: CanvasBounds[] = canvasCards(graph).map((node) => ({
    left: node.x - 9,
    top: node.y - 5,
    right: node.x + node.width + 9,
    bottom: node.y + node.height + 5
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
