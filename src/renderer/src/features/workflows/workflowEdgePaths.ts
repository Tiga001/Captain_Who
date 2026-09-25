import type { CanvasBounds, CanvasPoint, FlowBridge, FlowGeometry } from './workflowCanvasGeometry'

const length = (a: CanvasPoint, b: CanvasPoint) => Math.hypot(b.x - a.x, b.y - a.y)
function toward(a: CanvasPoint, b: CanvasPoint, distance: number): CanvasPoint {
  const ratio = distance / (length(a, b) || 1)
  return { x: a.x + (b.x - a.x) * ratio, y: a.y + (b.y - a.y) * ratio }
}
function corners(points: CanvasPoint[], radius: number): number[] {
  return points.map((p, i) =>
    i === 0 || i === points.length - 1
      ? 0
      : Math.min(radius, length(points[i - 1], p) / 2, length(p, points[i + 1]) / 2)
  )
}

/** Keep the hit target, rounded corners and visible crossing arcs on the same path. */
export function roundedFlowPath(
  points: CanvasPoint[],
  radius: number,
  bridges: FlowBridge[] = []
): string {
  const trim = corners(points, radius)
  let path = `M ${points[0].x} ${points[0].y}`
  for (let i = 0; i < points.length - 1; i++) {
    const a = points[i],
      b = points[i + 1]
    const local = bridges
      .filter((bridge) => bridge.segment === i)
      .sort((left, right) => (left.point.y - right.point.y) * Math.sign(b.y - a.y))
    for (const bridge of local) path += ` L ${bridge.start.x} ${bridge.start.y} ${bridge.arc}`
    const end = toward(b, a, trim[i + 1])
    path += ` L ${end.x} ${end.y}`
    if (i + 2 < points.length) {
      const next = toward(b, points[i + 2], trim[i + 1])
      path += ` Q ${b.x} ${b.y} ${next.x} ${next.y}`
    }
  }
  return path
}

interface Segment {
  id: string
  index: number
  a: CanvasPoint
  b: CanvasPoint
}

/** Only a proper crossing jumps. Shared trunks, junctions and rounded corners don't. */
export function addFlowCrossingBridges(
  geometries: Map<string, FlowGeometry | null>,
  obstacles: CanvasBounds[] = []
): Map<string, FlowGeometry | null> {
  const horizontal: Segment[] = [],
    vertical: Segment[] = []
  for (const [id, geometry] of geometries) {
    if (!geometry) continue
    const trim = corners(geometry.points, geometry.cornerRadius)
    for (let i = 0; i < geometry.points.length - 1; i++) {
      const a = toward(geometry.points[i], geometry.points[i + 1], trim[i])
      const b = toward(geometry.points[i + 1], geometry.points[i], trim[i + 1])
      if (length(a, b) < 16) continue
      const segment = { id, index: i, a, b }
      if (a.y === b.y) horizontal.push(segment)
      else if (a.x === b.x) vertical.push(segment)
    }
  }
  const byFlow = new Map<string, FlowBridge[]>()
  for (const segment of vertical) {
    const x = segment.a.x,
      top = Math.min(segment.a.y, segment.b.y),
      bottom = Math.max(segment.a.y, segment.b.y)
    const crossings = [
      ...new Set(
        horizontal
          .filter(
            (other) =>
              other.id !== segment.id &&
              x > Math.min(other.a.x, other.b.x) + 8 &&
              x < Math.max(other.a.x, other.b.x) - 8 &&
              other.a.y > top + 8 &&
              other.a.y < bottom - 8
          )
          .map((other) => other.a.y)
      )
    ].sort((a, b) => a - b)
    // Nearby parallel lines use one slightly taller bridge instead of overlapping arcs.
    const groups: number[][] = []
    for (const y of crossings) {
      const previous = groups.at(-1)
      if (previous && y - previous[previous.length - 1] < 16) previous.push(y)
      else groups.push([y])
    }
    for (const group of groups) {
      const radius = 6,
        y = (group[0] + group[group.length - 1]) / 2
      const halfHeight = (group[group.length - 1] - group[0]) / 2 + radius
      const clear = (direction: number) =>
        !obstacles.some(
          (card) =>
            card.left < Math.max(x, x + direction * (radius + 2)) &&
            card.right > Math.min(x, x + direction * (radius + 2)) &&
            card.top < y + halfHeight + 2 &&
            card.bottom > y - halfHeight - 2
        )
      const direction = clear(1) ? 1 : clear(-1) ? -1 : 0
      if (!direction) continue
      const down = segment.b.y > segment.a.y
      const start = { x, y: y + (down ? -halfHeight : halfHeight) }
      const end = { x, y: y + (down ? halfHeight : -halfHeight) }
      const sweep = down === direction > 0 ? 1 : 0
      const arc = `A ${radius} ${halfHeight} 0 0 ${sweep} ${end.x} ${end.y}`
      const bridge: FlowBridge = {
        point: { x, y },
        radius,
        halfHeight,
        direction,
        segment: segment.index,
        start,
        arc,
        path: `M ${start.x} ${start.y} ${arc}`
      }
      const bridges = byFlow.get(segment.id) ?? []
      bridges.push(bridge)
      byFlow.set(segment.id, bridges)
    }
  }
  return new Map(
    [...geometries].map(([id, geometry]) => {
      if (!geometry) return [id, null]
      const bridges = byFlow.get(id) ?? []
      return [
        id,
        {
          ...geometry,
          bridges,
          path: roundedFlowPath(geometry.points, geometry.cornerRadius, bridges)
        }
      ]
    })
  )
}
