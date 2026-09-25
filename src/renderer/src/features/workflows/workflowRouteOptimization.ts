import type { WorkflowFlow } from '@mycopilot/protocol'
import type { CanvasGraph, CanvasPoint, FlowGeometry } from './workflowCanvasGeometry'
import { anchorPoint } from './workflowAnchors'
import { roundedFlowPath, flowLabelPosition } from './workflowEdgePaths'
import { segmentIntersectsCard, simplifyRoute, type RoutingCard } from './workflowRouting'

const same = (a: CanvasPoint, b: CanvasPoint) => a.x === b.x && a.y === b.y
const length = (points: CanvasPoint[]) =>
  points
    .slice(1)
    .reduce((sum, p, i) => sum + Math.abs(p.x - points[i].x) + Math.abs(p.y - points[i].y), 0)
const segments = (points: CanvasPoint[]) => points.slice(1).map((b, i) => ({ a: points[i], b }))
const inside = (value: number, a: number, b: number) =>
  value > Math.min(a, b) && value < Math.max(a, b)
const within = (value: number, a: number, b: number) =>
  value >= Math.min(a, b) && value <= Math.max(a, b)

/** Count contacts as well as crossings: moving a bend onto another line must not hide a conflict. */
export function routeConflicts(a: CanvasPoint[], b: CanvasPoint[]) {
  const shared = [a[0], a.at(-1)!].filter((p) => same(p, b[0]) || same(p, b.at(-1)!))
  const crossings = new Set<string>()
  let overlap = 0
  for (const first of segments(a))
    for (const second of segments(b)) {
      const h1 = first.a.y === first.b.y,
        h2 = second.a.y === second.b.y
      if (h1 === h2) {
        const aligned = h1 ? first.a.y === second.a.y : first.a.x === second.a.x
        if (!aligned || shared.length) continue // Keep intentional fan-out/fan-in trunks.
        const axis = h1 ? 'x' : 'y'
        overlap += Math.max(
          0,
          Math.min(
            Math.max(first.a[axis], first.b[axis]),
            Math.max(second.a[axis], second.b[axis])
          ) -
            Math.max(
              Math.min(first.a[axis], first.b[axis]),
              Math.min(second.a[axis], second.b[axis])
            )
        )
        continue
      }
      const h = h1 ? first : second,
        v = h1 ? second : first
      const point = { x: v.a.x, y: h.a.y }
      if (!within(point.x, h.a.x, h.b.x) || !within(point.y, v.a.y, v.b.y)) continue
      if (shared.some((p) => same(p, point))) continue
      if (shared.length && !(inside(point.x, h.a.x, h.b.x) && inside(point.y, v.a.y, v.b.y)))
        continue
      crossings.add(`${point.x}:${point.y}`)
    }
  return { crossings: crossings.size, overlap }
}

function candidates(
  graph: CanvasGraph,
  flow: WorkflowFlow,
  original: FlowGeometry,
  cards: RoutingCard[],
  others: FlowGeometry[]
): CanvasPoint[][] {
  const source = anchorPoint(graph, flow.source, 'source', flow.sourceAnchor)!
  const target = anchorPoint(graph, flow.target, 'target', flow.targetAnchor)!
  const from = flow.source.kind === 'boundary' ? 'boundary:input' : `node:${flow.source.nodeId}`
  const to = flow.target.kind === 'boundary' ? 'boundary:output' : `node:${flow.target.nodeId}`
  const normals = { left: [-1, 0], right: [1, 0], top: [0, -1], bottom: [0, 1] } as const
  const start = original.start,
    end = original.end
  const limit = length(original.points) * 1.6 + 120
  const corridor = (axis: 'x' | 'y') => {
    const originalValues = original.points.map((p) => p[axis])
    const low = Math.min(...originalValues) - limit / 2,
      high = Math.max(...originalValues) + limit / 2
    const values = new Set([
      (start[axis] + end[axis]) / 2,
      low,
      high,
      ...originalValues.flatMap((v) => [v - 14, v + 14]),
      ...cards.flatMap((c) =>
        axis === 'x' ? [c.left - 18, c.right + 18] : [c.top - 18, c.bottom + 18]
      ),
      ...others.flatMap((g) => g.points.flatMap((p) => [p[axis] - 14, p[axis] + 14]))
    ])
    const cost = (v: number) => Math.abs(v - start[axis]) + Math.abs(v - end[axis])
    const ranked = [...values]
      .filter((v) => v >= low && v <= high)
      .sort((a, b) => cost(a) - cost(b) || a - b)
    // Outer lanes remain available even when many inner coordinates are occupied.
    const min = Math.min(start[axis], end[axis]),
      max = Math.max(start[axis], end[axis])
    return [
      ...new Set([
        ...ranked.filter((v) => v >= min && v <= max).slice(0, 6),
        ...ranked.filter((v) => v < min).slice(0, 4),
        ...ranked.filter((v) => v > max).slice(0, 4)
      ])
    ]
  }
  const xs = corridor('x'),
    ys = corridor('y')
  const result = new Map<string, CanvasPoint[]>()
  const obstacles = cards.map((c) => ({
    ...c,
    left: c.left - 4,
    right: c.right + 4,
    top: c.top - 4,
    bottom: c.bottom + 4
  }))
  const add = (points: CanvasPoint[]) => {
    const route = simplifyRoute(points)
    if (route.length < 2 || route.length > original.points.length + 4 || length(route) > limit)
      return
    const second = route[1],
      previous = route.at(-2)!
    const outgoing = normals[source.side],
      incoming = normals[target.side]
    if (
      Math.sign(second.x - start.x) !== outgoing[0] ||
      Math.sign(second.y - start.y) !== outgoing[1] ||
      Math.sign(previous.x - end.x) !== incoming[0] ||
      Math.sign(previous.y - end.y) !== incoming[1]
    )
      return
    for (let i = 1; i < route.length; i++) {
      const a = route[i - 1],
        b = route[i]
      if (a.x !== b.x && a.y !== b.y) return
      if (
        obstacles.some(
          (c) =>
            !(i === 1 && c.key === from) &&
            !(i === route.length - 1 && c.key === to) &&
            segmentIntersectsCard(a, b, c)
        )
      )
        return
      // A U-turn retraces its own line and cannot improve readability.
      if (i > 1) {
        const previous = route[i - 2]
        if ((a.x - previous.x) * (b.x - a.x) + (a.y - previous.y) * (b.y - a.y) < 0) return
      }
    }
    result.set(JSON.stringify(route), route)
  }
  for (const gap of [18, 34]) {
    const a = {
      x: start.x + normals[source.side][0] * gap,
      y: start.y + normals[source.side][1] * gap
    }
    const b = { x: end.x + normals[target.side][0] * gap, y: end.y + normals[target.side][1] * gap }
    add([start, a, { x: b.x, y: a.y }, b, end])
    add([start, a, { x: a.x, y: b.y }, b, end])
    for (const x of xs) add([start, a, { x, y: a.y }, { x, y: b.y }, b, end])
    for (const y of ys) add([start, a, { x: a.x, y }, { x: b.x, y }, b, end])
    for (const x of xs)
      for (const y of ys) {
        add([start, a, { x, y: a.y }, { x, y }, { x: b.x, y }, b, end])
        add([start, a, { x: a.x, y }, { x, y }, { x, y: b.y }, b, end])
      }
  }
  return [...result.values()]
}

/** Bounded coordinate descent. Accept only improvements against every other current route. */
export function optimizeWorkflowRoutes(
  graph: CanvasGraph,
  initial: Map<string, FlowGeometry | null>,
  cards: RoutingCard[]
) {
  if (graph.flows.length < 2) return initial
  const routes = new Map(initial)
  const sorted = [...graph.flows].sort((a, b) => a.id.localeCompare(b.id))
  const score = (points: CanvasPoint[], others: FlowGeometry[]) => {
    let crossings = 0,
      overlap = 0
    for (const other of others) {
      const cost = routeConflicts(points, other.points)
      crossings += cost.crossings
      overlap += cost.overlap
    }
    return { crossings, overlap, cost: overlap * 6 + length(points) + (points.length - 2) * 24 }
  }
  const better = (a: ReturnType<typeof score>, b: ReturnType<typeof score>) =>
    a.overlap <= b.overlap + 0.01 &&
    (a.crossings < b.crossings || (a.crossings === b.crossings && a.cost < b.cost - 0.01))
  const maxAttempts = Math.max(8, Math.min(48, Math.floor(768 / graph.flows.length)))
  let attempts = 0
  for (let pass = 0; pass < 2; pass++) {
    let changed = false
    for (const flow of sorted) {
      const geometry = routes.get(flow.id)
      if (!geometry) continue
      const others = [...routes].filter(([id, g]) => id !== flow.id && g).map(([, g]) => g!)
      const current = score(geometry.points, others)
      if (!current.crossings && current.overlap < 1) continue
      if (attempts++ >= maxAttempts) return routes
      let best = geometry.points,
        bestScore = current
      for (const points of candidates(graph, flow, initial.get(flow.id)!, cards, others)) {
        const cost = score(points, others)
        if (better(cost, bestScore)) {
          best = points
          bestScore = cost
        }
      }
      if (best === geometry.points) continue
      const radius = Math.min(geometry.cornerRadius, 8)
      routes.set(flow.id, {
        ...geometry,
        points: best,
        cornerRadius: radius,
        bridges: [],
        path: roundedFlowPath(best, radius),
        label: flowLabelPosition(best)
      })
      changed = true
    }
    if (!changed) break
  }
  return routes
}
