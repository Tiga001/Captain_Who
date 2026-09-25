import type { CanvasPoint } from './workflowCanvasGeometry'

export interface RoutingCard {
  key: string
  left: number
  right: number
  top: number
  bottom: number
}
interface RouteOptions {
  start: CanvasPoint
  end: CanvasPoint
  from: string
  to: string
  cards: RoutingCard[]
  preferredX: number
  lane: number
  laneCount: number
}
const same = (a: CanvasPoint, b: CanvasPoint) => a.x === b.x && a.y === b.y
const distance = (a: CanvasPoint, b: CanvasPoint) => Math.abs(a.x - b.x) + Math.abs(a.y - b.y)

function simplify(points: CanvasPoint[]): CanvasPoint[] {
  const result: CanvasPoint[] = []
  for (const point of points) {
    if (result.length && same(result[result.length - 1], point)) continue
    const a = result.at(-2),
      b = result.at(-1)
    // Only merge segments travelling in the same direction, never a U-turn.
    if (
      a &&
      b &&
      ((a.x === b.x && b.x === point.x && (b.y - a.y) * (point.y - b.y) >= 0) ||
        (a.y === b.y && b.y === point.y && (b.x - a.x) * (point.x - b.x) >= 0))
    )
      result.pop()
    result.push(point)
  }
  return result
}

function intersects(a: CanvasPoint, b: CanvasPoint, card: RoutingCard): boolean {
  if (a.x === b.x)
    return (
      a.x > card.left &&
      a.x < card.right &&
      Math.max(a.y, b.y) > card.top &&
      Math.min(a.y, b.y) < card.bottom
    )
  return (
    a.y > card.top &&
    a.y < card.bottom &&
    Math.max(a.x, b.x) > card.left &&
    Math.min(a.x, b.x) < card.right
  )
}
/** Search the rectilinear visibility grid only when simple local corridors are blocked. */
function search(start: CanvasPoint, end: CanvasPoint, cards: RoutingCard[]): CanvasPoint[] | null {
  const xs = [...new Set([start.x, end.x, ...cards.flatMap((c) => [c.left, c.right])])].sort(
    (a, b) => a - b
  )
  const ys = [...new Set([start.y, end.y, ...cards.flatMap((c) => [c.top, c.bottom])])].sort(
    (a, b) => a - b
  )
  const width = xs.length
  // Grid coordinates include every obstacle boundary. Mark blocked grid edges once,
  // instead of scanning every card for every step of the search.
  const horizontal = new Uint8Array(width * ys.length)
  const vertical = new Uint8Array(width * ys.length)
  for (const card of cards) {
    const left = xs.indexOf(card.left),
      right = xs.indexOf(card.right)
    const top = ys.indexOf(card.top),
      bottom = ys.indexOf(card.bottom)
    for (let y = top + 1; y < bottom; y++) horizontal.fill(1, y * width + left, y * width + right)
    for (let y = top; y < bottom; y++) vertical.fill(1, y * width + left + 1, y * width + right)
  }
  const point = (id: number): CanvasPoint => ({ x: xs[id % width], y: ys[Math.floor(id / width)] })
  const first = ys.indexOf(start.y) * width + xs.indexOf(start.x)
  const last = ys.indexOf(end.y) * width + xs.indexOf(end.x)
  // Include arrival direction in the state so fewer bends win over staircases.
  const costs = new Float64Array(width * ys.length * 2).fill(Infinity)
  const parents = new Int32Array(costs.length).fill(-1)
  costs[first * 2] = 0
  const heap: Array<{ state: number; cost: number; rank: number }> = []
  const push = (item: (typeof heap)[number]) => {
    let i = heap.length
    heap.push(item)
    while (i) {
      const parent = Math.floor((i - 1) / 2)
      if (heap[parent].rank <= item.rank) break
      heap[i] = heap[parent]
      i = parent
    }
    heap[i] = item
  }
  const pop = () => {
    const first = heap[0],
      tail = heap.pop()!
    if (heap.length) {
      let i = 0
      while (i * 2 + 1 < heap.length) {
        let next = i * 2 + 1
        if (next + 1 < heap.length && heap[next + 1].rank < heap[next].rank) next++
        if (heap[next].rank >= tail.rank) break
        heap[i] = heap[next]
        i = next
      }
      heap[i] = tail
    }
    return first
  }
  push({ state: first * 2, cost: 0, rank: distance(start, end) })
  while (heap.length) {
    const current = pop()
    if (costs[current.state] !== current.cost) continue
    const id = Math.floor(current.state / 2)
    if (id === last) {
      const points = [point(id)]
      let state = current.state
      while (parents[state] !== -1) {
        state = parents[state]
        points.push(point(Math.floor(state / 2)))
      }
      return simplify(points.reverse())
    }
    const x = id % width,
      y = Math.floor(id / width)
    for (let step = 0; step < 4; step++) {
      const nx = x + (step === 0 ? -1 : step === 1 ? 1 : 0)
      const ny = y + (step === 2 ? -1 : step === 3 ? 1 : 0)
      if (nx < 0 || nx >= width || ny < 0 || ny >= ys.length) continue
      const next = ny * width + nx,
        direction = step < 2 ? 0 : 1
      if ((direction === 0 ? horizontal : vertical)[Math.min(id, next)]) continue
      const nextCost =
        current.cost +
        Math.abs(xs[x] - xs[nx]) +
        Math.abs(ys[y] - ys[ny]) +
        (direction !== current.state % 2 ? 24 : 0)
      const state = next * 2 + direction
      if (nextCost >= costs[state]) continue
      costs[state] = nextCost
      parents[state] = current.state
      push({
        state,
        cost: nextCost,
        rank: nextCost + Math.abs(xs[nx] - end.x) + Math.abs(ys[ny] - end.y)
      })
    }
  }
  return null
}

export function routeWorkflowFlow(options: RouteOptions): {
  points: CanvasPoint[]
  radius: number
} {
  const { start, end, from, to, preferredX, lane, laneCount } = options
  const forward = end.x > start.x
  const offset = lane ? Math.ceil(lane / 2) * 16 * (lane % 2 ? 1 : -1) : 0
  for (const clearance of [20, 8, 0]) {
    const portGap = forward ? Math.min(clearance, (end.x - start.x) / 3) : clearance
    const cards = options.cards.map((card) => {
      const margin = card.key === from || card.key === to ? portGap : clearance
      return {
        ...card,
        left: card.left - margin,
        right: card.right + margin,
        top: card.top - clearance - lane * 16,
        bottom: card.bottom + clearance + lane * 16
      }
    })
    const outX = Math.max(
      start.x + Math.max(portGap, 1),
      cards.find((card) => card.key === from)!.right
    )
    const inX = Math.min(end.x - Math.max(portGap, 1), cards.find((card) => card.key === to)!.left)
    const candidates: CanvasPoint[][] = []
    const add = (points: CanvasPoint[]) => {
      const route = simplify(points)
      if (route.length < 2 || route[1].x <= start.x || route.at(-2)!.x >= end.x) return
      if (
        route.some(
          (point, i) =>
            i > 0 &&
            cards.some(
              (card) =>
                !(i === 1 && card.key === from) &&
                !(i === route.length - 1 && card.key === to) &&
                intersects(route[i - 1], point, card)
            )
        )
      )
        return
      candidates.push(route)
    }
    if (forward) {
      const clamp = (x: number) => Math.max(outX, Math.min(inX, x))
      // Shared fan-out/fan-in trunks keep adjacent branches aligned.
      const middle =
        laneCount > 1 ? outX + ((inX - outX) * (lane + 1)) / (laneCount + 1) : clamp(preferredX)
      const lanes = [
        ...new Set([
          middle,
          clamp((start.x + end.x) / 2 + offset),
          outX,
          inX,
          ...cards.flatMap((card) => [card.left, card.right]).filter((x) => x >= outX && x <= inX)
        ])
      ]
      for (const x of lanes) {
        if (lane && start.y === end.y) continue
        add([start, { x, y: start.y }, { x, y: end.y }, end])
        // A clear preferred corridor is already a shortest, two-bend path.
        if (candidates.length) return { points: candidates[0], radius: Math.min(12, clearance / 2) }
      }
    }
    const ys = [
      ...new Set([
        start.y + offset,
        end.y + offset,
        ...cards.flatMap((card) => [card.top, card.bottom])
      ])
    ].sort(
      (a, b) =>
        Math.abs(a - start.y) + Math.abs(a - end.y) - Math.abs(b - start.y) - Math.abs(b - end.y) ||
        a - b
    )
    if (!forward) {
      // Return links skirt the whole span of intervening columns, keeping the main flow clear.
      const span = cards.filter((card) => card.right >= inX && card.left <= outX)
      const outer = [
        Math.min(...span.map((card) => card.top)),
        Math.max(...span.map((card) => card.bottom))
      ].sort(
        (a, b) =>
          Math.abs(a - start.y) +
            Math.abs(a - end.y) -
            Math.abs(b - start.y) -
            Math.abs(b - end.y) || a - b
      )
      ys.unshift(...outer)
    }
    for (const y of ys) {
      if (lane && start.y === end.y && y === start.y) continue
      add([
        start,
        { x: outX, y: start.y },
        { x: outX, y },
        { x: inX, y },
        { x: inX, y: end.y },
        end
      ])
      // Corridors are already ordered by length and have the same bend count.
      if (candidates.length) return { points: candidates[0], radius: Math.min(12, clearance / 2) }
    }
    // The stubs pass through their own card's clearance, but never another card.
    if (
      cards.some((card) => card.key !== from && intersects(start, { x: outX, y: start.y }, card)) ||
      cards.some((card) => card.key !== to && intersects({ x: inX, y: end.y }, end, card))
    )
      continue
    const inner = search({ x: outX, y: start.y }, { x: inX, y: end.y }, cards)
    if (inner)
      return { points: simplify([start, ...inner, end]), radius: Math.min(12, clearance / 2) }
  }
  // Overlapping cards can cover a port completely. Keep the edge attached while dragging.
  return {
    points: simplify([
      start,
      { x: (start.x + end.x) / 2, y: start.y },
      { x: (start.x + end.x) / 2, y: end.y },
      end
    ]),
    radius: 0
  }
}

/** Arbitrary card sides: leave/enter along the outward normal, then route around obstacles. */
export function routeAnchoredFlow(
  options: RouteOptions & {
    sourceSide: 'left' | 'right' | 'top' | 'bottom'
    targetSide: 'left' | 'right' | 'top' | 'bottom'
  }
): { points: CanvasPoint[]; radius: number } {
  if (options.sourceSide === 'right' && options.targetSide === 'left')
    return routeWorkflowFlow(options)
  const { start, end, from, to } = options
  const normals = { left: [-1, 0], right: [1, 0], top: [0, -1], bottom: [0, 1] } as const
  const stub = (point: CanvasPoint, side: keyof typeof normals, amount: number) => ({
    x: point.x + normals[side][0] * amount,
    y: point.y + normals[side][1] * amount
  })
  for (const clearance of [20, 8, 0]) {
    const cards = options.cards.map((card) => ({
      ...card,
      left: card.left - clearance,
      right: card.right + clearance,
      top: card.top - clearance,
      bottom: card.bottom + clearance
    }))
    const a = stub(start, options.sourceSide, clearance + 3)
    const b = stub(end, options.targetSide, clearance + 3)
    if (
      cards.some((card) => card.key !== from && intersects(start, a, card)) ||
      cards.some((card) => card.key !== to && intersects(b, end, card))
    )
      continue
    const candidates: CanvasPoint[][] = [
      [a, { x: b.x, y: a.y }, b],
      [a, { x: a.x, y: b.y }, b]
    ]
    if (same(a, b)) {
      const c = cards.find((card) => card.key === from)!
      const ring = [
        { x: c.left - 3, y: c.top - 3 },
        { x: c.right + 3, y: c.top - 3 },
        { x: c.right + 3, y: c.bottom + 3 },
        { x: c.left - 3, y: c.bottom + 3 }
      ]
      const side = options.sourceSide
      const i = side === 'top' ? 0 : side === 'right' ? 1 : side === 'bottom' ? 2 : 3
      candidates.splice(0, candidates.length, [
        a,
        ...ring.slice(i),
        ...ring.slice(0, i),
        ring[i],
        a
      ])
    }
    for (const route of candidates) {
      if (
        route.some(
          (p, i) =>
            i > 0 &&
            ((route[i - 1].x !== p.x && route[i - 1].y !== p.y) ||
              cards.some((card) => intersects(route[i - 1], p, card)))
        )
      )
        continue
      if (same(a, b) && route.length < 4) continue
      return { points: simplify([start, ...route, end]), radius: Math.min(10, clearance / 2) }
    }
    const route = same(a, b) ? null : search(a, b, cards)
    if (route)
      return { points: simplify([start, ...route, end]), radius: Math.min(10, clearance / 2) }
  }
  const a = stub(start, options.sourceSide, 3),
    b = stub(end, options.targetSide, 3)
  return { points: simplify([start, a, { x: b.x, y: a.y }, b, end]), radius: 0 }
}

export { simplify as simplifyRoute, intersects as segmentIntersectsCard }
