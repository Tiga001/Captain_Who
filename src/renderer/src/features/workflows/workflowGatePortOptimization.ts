import type { WorkflowFlow } from '@mycopilot/protocol'
import type { CanvasGraph, FlowGeometry } from './workflowCanvasGeometry'
import { routeConflicts } from './workflowRouteOptimization'

const routeLength = (route: FlowGeometry) =>
  route.points
    .slice(1)
    .reduce(
      (sum, p, i) => sum + Math.abs(p.x - route.points[i].x) + Math.abs(p.y - route.points[i].y),
      0
    )

/** Position sorting is the starting point; bounded swaps account for obstacles and return loops. */
export function improveGateAnchorOrder(
  definition: CanvasGraph,
  resolved: CanvasGraph,
  initial: Map<string, FlowGeometry | null>,
  route: (graph: CanvasGraph, flow: WorkflowFlow) => FlowGeometry | null
) {
  let graph = resolved
  const routes = new Map(initial)
  const limit = Math.max(4, Math.min(32, Math.floor(512 / Math.max(1, graph.flows.length))))
  let attempts = 0
  let comparisons = 0
  const score = (changed: Map<string, FlowGeometry | null>) => {
    const seen = new Set<string>()
    let crossings = 0,
      overlap = 0,
      length = 0
    for (const [id, geometry] of changed) {
      if (!geometry) continue
      seen.add(id)
      length += routeLength(geometry) + Math.max(0, geometry.points.length - 2) * 24
      for (const [otherId, original] of routes) {
        if (seen.has(otherId)) continue
        const other = changed.get(otherId) ?? original
        if (!other) continue
        const cost = routeConflicts(geometry.points, other.points)
        crossings += cost.crossings
        overlap += cost.overlap
      }
    }
    return { crossings, overlap, length }
  }
  const better = (a: ReturnType<typeof score>, b: ReturnType<typeof score>) =>
    a.overlap <= b.overlap + 0.01 &&
    a.length <= b.length * 1.6 + 120 &&
    (a.crossings < b.crossings ||
      (a.crossings === b.crossings && a.overlap * 6 + a.length < b.overlap * 6 + b.length - 0.01))

  for (const node of [...definition.nodes].sort((a, b) => a.id.localeCompare(b.id))) {
    if (node.kind !== 'inputGate' && node.kind !== 'outputGate') continue
    const end = node.kind === 'inputGate' ? 'target' : 'source'
    const key = end === 'target' ? 'targetAnchor' : 'sourceAnchor'
    const movable = definition.flows
      .filter((flow) => {
        const ep = flow[end]
        return !flow[key] && ep.kind === 'node' && ep.nodeId === node.id
      })
      .map((flow) => flow.id)
      .sort()
    for (let i = 0; i < movable.length; i++) {
      for (let j = i + 1; j < movable.length; j++) {
        if (comparisons++ >= limit * 4) return { graph, routes }
        const ids = [movable[i], movable[j]]
        const a = graph.flows.find((flow) => flow.id === ids[0])!
        const b = graph.flows.find((flow) => flow.id === ids[1])!
        const before = score(new Map(ids.map((id) => [id, routes.get(id) ?? null])))
        if (!before.crossings && before.overlap < 1) continue
        if (attempts++ >= limit) return { graph, routes }
        const replacements = new Map([
          [a.id, { ...a, [key]: b[key] }],
          [b.id, { ...b, [key]: a[key] }]
        ])
        const candidate = {
          ...graph,
          flows: graph.flows.map((flow) => replacements.get(flow.id) ?? flow)
        }
        const changed = new Map([...replacements].map(([id, flow]) => [id, route(candidate, flow)]))
        if ([...changed.values()].some((geometry) => !geometry)) continue
        if (!better(score(changed), before)) continue
        graph = candidate
        for (const [id, geometry] of changed) routes.set(id, geometry)
      }
    }
  }
  return { graph, routes }
}
