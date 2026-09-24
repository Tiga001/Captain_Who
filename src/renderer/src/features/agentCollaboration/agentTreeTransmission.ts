export interface AgentTreeTransmission {
  id: string
  sourceAgentId: string | null
  targetAgentId: string | null
  kind: 'message' | 'task' | 'user_message' | 'completion'
  receivedAt: number
}

export interface TreePoint {
  x: number
  y: number
}
export interface TreeNodeBounds {
  id: string | null
  parentId: string | null
  left: number
  top: number
  right: number
  bottom: number
}

export interface AgentTreeTransmissionRoute {
  path: string
  kind: 'branch' | 'arc'
  points: TreePoint[]
}

const CLEARANCE = 7
const OBSTACLE_MARGIN = 2
const centerX = (node: TreeNodeBounds) => (node.left + node.right) / 2
const centerY = (node: TreeNodeBounds) => (node.top + node.bottom) / 2
const distance = (a: TreePoint, b: TreePoint) => Math.abs(a.x - b.x) + Math.abs(a.y - b.y)
const pair = ({ x, y }: TreePoint) => `${Number(x.toFixed(2))} ${Number(y.toFixed(2))}`

function roundedPath(points: TreePoint[]): string {
  let path = `M ${pair(points[0])}`
  for (let index = 1; index < points.length - 1; index += 1) {
    const previous = points[index - 1]
    const current = points[index]
    const next = points[index + 1]
    const radius = Math.min(12, distance(previous, current) / 2, distance(current, next) / 2)
    const before = {
      x: current.x + Math.sign(previous.x - current.x) * radius,
      y: current.y + Math.sign(previous.y - current.y) * radius
    }
    const after = {
      x: current.x + Math.sign(next.x - current.x) * radius,
      y: current.y + Math.sign(next.y - current.y) * radius
    }
    path += ` L ${pair(before)} Q ${pair(current)} ${pair(after)}`
  }
  return `${path} L ${pair(points.at(-1)!)}`
}

function simplify(points: TreePoint[]): TreePoint[] {
  return points.filter((point, index) => {
    const previous = points[index - 1]
    const next = points[index + 1]
    if (previous && distance(previous, point) < 0.01) return false
    return (
      !previous ||
      !next ||
      !(
        (previous.x === point.x && point.x === next.x) ||
        (previous.y === point.y && point.y === next.y)
      )
    )
  })
}

function ports(node: TreeNodeBounds) {
  return [
    {
      border: { x: centerX(node), y: node.top },
      point: { x: centerX(node), y: node.top - CLEARANCE }
    },
    {
      border: { x: centerX(node), y: node.bottom },
      point: { x: centerX(node), y: node.bottom + CLEARANCE }
    },
    {
      border: { x: node.left, y: centerY(node) },
      point: { x: node.left - CLEARANCE, y: centerY(node) }
    },
    {
      border: { x: node.right, y: centerY(node) },
      point: { x: node.right + CLEARANCE, y: centerY(node) }
    }
  ]
}

function intersects(a: TreePoint, b: TreePoint, node: TreeNodeBounds) {
  const left = node.left - OBSTACLE_MARGIN
  const right = node.right + OBSTACLE_MARGIN
  const top = node.top - OBSTACLE_MARGIN
  const bottom = node.bottom + OBSTACLE_MARGIN
  return a.y === b.y
    ? a.y > top && a.y < bottom && Math.max(a.x, b.x) > left && Math.min(a.x, b.x) < right
    : a.x > left && a.x < right && Math.max(a.y, b.y) > top && Math.min(a.y, b.y) < bottom
}

function curvedRoute(
  source: TreeNodeBounds,
  target: TreeNodeBounds,
  nodes: readonly TreeNodeBounds[],
  width: number,
  height: number
): AgentTreeTransmissionRoute | null {
  const normals = [
    { x: 0, y: -1 },
    { x: 0, y: 1 },
    { x: -1, y: 0 },
    { x: 1, y: 0 }
  ]
  const sourcePorts = ports(source)
  const targetPorts = ports(target)
  const candidates: { route: AgentTreeTransmissionRoute; length: number }[] = []
  // Matching outward-facing ports create a visibly curved arch even for same-column ancestors.
  // Try compact arches first; when cards block them, the orthogonal router below finds a safe lane.
  for (const side of [0, 1, 2, 3]) {
    const start = sourcePorts[side].border
    const end = targetPorts[side].border
    const normal = normals[side]
    for (const offset of [16, 28, 48, 80, 128, 192]) {
      const first = { x: start.x + normal.x * offset, y: start.y + normal.y * offset }
      const second = { x: end.x + normal.x * offset, y: end.y + normal.y * offset }
      const points: TreePoint[] = []
      let safe = true
      let length = 0
      for (let index = 0; index <= 80; index += 1) {
        const t = index / 80
        const inverse = 1 - t
        const point = {
          x:
            inverse ** 3 * start.x +
            3 * inverse ** 2 * t * first.x +
            3 * inverse * t ** 2 * second.x +
            t ** 3 * end.x,
          y:
            inverse ** 3 * start.y +
            3 * inverse ** 2 * t * first.y +
            3 * inverse * t ** 2 * second.y +
            t ** 3 * end.y
        }
        if (
          point.x < 1 ||
          point.x > width - 1 ||
          point.y < 1 ||
          point.y > height - 1 ||
          nodes.some((node) => {
            const margin = node === source || node === target ? 0 : OBSTACLE_MARGIN
            return (
              point.x > node.left - margin &&
              point.x < node.right + margin &&
              point.y > node.top - margin &&
              point.y < node.bottom + margin
            )
          })
        ) {
          safe = false
          break
        }
        if (points.length)
          length += Math.hypot(point.x - points.at(-1)!.x, point.y - points.at(-1)!.y)
        points.push(point)
      }
      if (safe)
        candidates.push({
          length,
          route: {
            kind: 'arc',
            points,
            path: `M ${pair(start)} C ${pair(first)} ${pair(second)} ${pair(end)}`
          }
        })
    }
  }
  return candidates.sort((a, b) => a.length - b.length)[0]?.route ?? null
}

/** Route around real node rectangles, never through their avatars or labels. */
function obstacleRoute(
  source: TreeNodeBounds,
  target: TreeNodeBounds,
  nodes: readonly TreeNodeBounds[],
  width: number,
  height: number
): TreePoint[] | null {
  const inside = ({ x, y }: TreePoint) => x >= 1 && y >= 1 && x <= width - 1 && y <= height - 1
  const sourcePorts = ports(source).filter((port) => inside(port.point))
  const targetPorts = ports(target).filter((port) => inside(port.point))
  if (!sourcePorts.length || !targetPorts.length) return null
  const xs = [
    ...new Set([
      1,
      width - 1,
      ...nodes.flatMap((node) => [node.left - CLEARANCE, node.right + CLEARANCE]),
      ...[...sourcePorts, ...targetPorts].map(({ point }) => point.x)
    ])
  ]
    .filter((x) => x >= 1 && x <= width - 1)
    .sort((a, b) => a - b)
  const ys = [
    ...new Set([
      1,
      height - 1,
      ...nodes.flatMap((node) => [node.top - CLEARANCE, node.bottom + CLEARANCE]),
      ...[...sourcePorts, ...targetPorts].map(({ point }) => point.y)
    ])
  ]
    .filter((y) => y >= 1 && y <= height - 1)
    .sort((a, b) => a - b)
  const indexOf = (point: TreePoint) => ys.indexOf(point.y) * xs.length + xs.indexOf(point.x)
  const pointOf = (index: number) => ({
    x: xs[index % xs.length],
    y: ys[Math.floor(index / xs.length)]
  })
  const targets = new Map(targetPorts.map((port) => [indexOf(port.point), port.border]))
  const starts = new Map(sourcePorts.map((port) => [indexOf(port.point), port.border]))
  const costs = new Map<number, number>()
  const previous = new Map<number, number>()
  const frontier: { index: number; score: number; cost: number }[] = []
  const enqueue = (index: number, cost: number) => {
    const point = pointOf(index)
    const score = cost + Math.min(...targetPorts.map((port) => distance(point, port.point)))
    let low = 0
    let high = frontier.length
    while (low < high) {
      const middle = (low + high) >>> 1
      if (frontier[middle].score < score) low = middle + 1
      else high = middle
    }
    frontier.splice(low, 0, { index, score, cost })
  }
  for (const index of starts.keys()) {
    costs.set(index, 0)
    enqueue(index, 0)
  }
  while (frontier.length) {
    const current = frontier.shift()!
    if (current.cost !== costs.get(current.index)) continue
    if (targets.has(current.index)) {
      const reversed = [targets.get(current.index)!, pointOf(current.index)]
      let cursor = current.index
      while (previous.has(cursor)) {
        cursor = previous.get(cursor)!
        reversed.push(pointOf(cursor))
      }
      reversed.push(starts.get(cursor)!)
      return simplify(reversed.reverse())
    }
    const x = current.index % xs.length
    const y = Math.floor(current.index / xs.length)
    const neighbors = [
      x > 0 ? current.index - 1 : -1,
      x + 1 < xs.length ? current.index + 1 : -1,
      y > 0 ? current.index - xs.length : -1,
      y + 1 < ys.length ? current.index + xs.length : -1
    ]
    const point = pointOf(current.index)
    for (const next of neighbors) {
      if (next < 0) continue
      const nextPoint = pointOf(next)
      if (nodes.some((node) => intersects(point, nextPoint, node))) continue
      const cost = current.cost + distance(point, nextPoint)
      if (cost >= (costs.get(next) ?? Infinity)) continue
      costs.set(next, cost)
      previous.set(next, current.index)
      enqueue(next, cost)
    }
  }
  return null
}

export function routeAgentTreeTransmission(
  transmission: AgentTreeTransmission,
  nodes: readonly TreeNodeBounds[],
  width: number,
  height: number
): AgentTreeTransmissionRoute | null {
  if (transmission.sourceAgentId === transmission.targetAgentId) return null
  const source = nodes.find((node) => node.id === transmission.sourceAgentId)
  const target = nodes.find((node) => node.id === transmission.targetAgentId)
  if (!source || !target) return null
  const forward = target.id !== null && target.parentId === source.id
  const backward = source.id !== null && source.parentId === target.id
  if (forward || backward) {
    const parent = forward ? source : target
    const child = forward ? target : source
    const middleY = (parent.bottom + child.top) / 2
    const points = simplify([
      { x: centerX(parent), y: parent.bottom },
      { x: centerX(parent), y: middleY },
      { x: centerX(child), y: middleY },
      { x: centerX(child), y: child.top }
    ])
    if (backward) points.reverse()
    return {
      kind: 'branch',
      points,
      path: points.map((point, index) => `${index ? 'L' : 'M'} ${pair(point)}`).join(' ')
    }
  }
  const curve = curvedRoute(source, target, nodes, width, height)
  if (curve) return curve
  const points = obstacleRoute(source, target, nodes, width, height)
  return points ? { kind: 'arc', points, path: roundedPath(points) } : null
}
