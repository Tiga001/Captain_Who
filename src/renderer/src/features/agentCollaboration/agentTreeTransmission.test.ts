import { describe, expect, it } from 'vitest'
import {
  routeAgentTreeTransmission,
  type AgentTreeTransmission,
  type TreeNodeBounds
} from './agentTreeTransmission'

const bounds = (
  id: string | null,
  parentId: string | null,
  left: number,
  top: number
): TreeNodeBounds => ({ id, parentId, left, top, right: left + 156, bottom: top + 48 })
const nodes = [
  bounds(null, null, 230, 8),
  bounds('root', null, 230, 80),
  bounds('a', 'root', 100, 152),
  bounds('b', 'root', 380, 152),
  bounds('c', 'a', 20, 224),
  bounds('d', 'a', 190, 224)
]
const event = (
  sourceAgentId: string | null,
  targetAgentId: string | null
): AgentTreeTransmission => ({
  id: 'test',
  sourceAgentId,
  targetAgentId,
  kind: 'message',
  receivedAt: 1
})

describe('agent tree transmission routing', () => {
  it('follows the existing branch elbows in the actual source-to-recipient direction', () => {
    const down = routeAgentTreeTransmission(event('root', 'a'), nodes, 580, 300)!
    expect(down.kind).toBe('branch')
    expect(down.points).toEqual([
      { x: 308, y: 128 },
      { x: 308, y: 140 },
      { x: 178, y: 140 },
      { x: 178, y: 152 }
    ])
    const up = routeAgentTreeTransmission(event('a', 'root'), nodes, 580, 300)!
    expect(up.points).toEqual([...down.points].reverse())
    expect(routeAgentTreeTransmission(event(null, 'root'), nodes, 580, 300)?.points).toEqual([
      { x: 308, y: 56 },
      { x: 308, y: 80 }
    ])
  })

  it.each([
    ['a', 'b'],
    ['root', 'c'],
    ['c', 'b']
  ] as const)('routes %s to %s around node rectangles with rounded bends', (from, to) => {
    const route = routeAgentTreeTransmission(event(from, to), nodes, 580, 300)!
    expect(route.kind).toBe('arc')
    expect(route.path).toMatch(/[CQ]/)
    const source = nodes.find((node) => node.id === from)!
    const target = nodes.find((node) => node.id === to)!
    const first = route.points[0]
    const last = route.points.at(-1)!
    expect(
      first.x === source.left ||
        first.x === source.right ||
        first.y === source.top ||
        first.y === source.bottom
    ).toBe(true)
    expect(
      last.x === target.left ||
        last.x === target.right ||
        last.y === target.top ||
        last.y === target.bottom
    ).toBe(true)
    if (route.path.includes('C')) {
      for (const point of route.points) {
        expect(
          nodes.some(
            (node) =>
              point.x > node.left &&
              point.x < node.right &&
              point.y > node.top &&
              point.y < node.bottom
          )
        ).toBe(false)
      }
      return
    }
    for (let index = 1; index < route.points.length; index += 1) {
      const a = route.points[index - 1]
      const b = route.points[index]
      for (const node of nodes) {
        const passesThrough =
          a.y === b.y
            ? a.y > node.top &&
              a.y < node.bottom &&
              Math.max(a.x, b.x) > node.left &&
              Math.min(a.x, b.x) < node.right
            : a.x > node.left &&
              a.x < node.right &&
              Math.max(a.y, b.y) > node.top &&
              Math.min(a.y, b.y) < node.bottom
        expect(passesThrough, `${from} → ${to} crosses ${node.id}`).toBe(false)
      }
    }
  })

  it('does not draw when either endpoint is hidden or both endpoints are identical', () => {
    expect(routeAgentTreeTransmission(event('root', 'hidden'), nodes, 580, 300)).toBeNull()
    expect(routeAgentTreeTransmission(event('a', 'a'), nodes, 580, 300)).toBeNull()
  })
})
