import { resolveWorkflowAnchors } from '../../features/workflows/workflowAnchorLayout'
import { routeConflicts } from '../../features/workflows/workflowRouteOptimization'
import { reviewLoopGraph } from './workflowRoutingFixtures'
import { anchorAtPoint, anchorPoint } from '../../features/workflows/workflowAnchors'
import { describe, expect, it } from 'vitest'
import {
  parseWorkflowDefinition,
  type WorkflowDefinition,
  type WorkflowFlow
} from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import {
  NODE_HEIGHT,
  NODE_WIDTH,
  workflowNodeSize
} from '../../features/workflows/workflowAuthoring'
import {
  canvasOrigin,
  fitViewport,
  flowGeometry,
  graphBounds,
  graphFlowGeometries,
  zoomViewport,
  type CanvasPoint,
  type FlowGeometry
} from '../../features/workflows/workflowCanvasGeometry'
import { addFlowCrossingBridges, roundedFlowPath } from '../../features/workflows/workflowEdgePaths'

function connection(id: string, source: string | null, target: string | null): WorkflowFlow {
  return {
    id,
    name: id,
    source: source === null ? { kind: 'boundary' } : { kind: 'node', nodeId: source },
    target: { kind: 'node', nodeId: target ?? 'result' }
  }
}

function routingGraph(
  nodes: Array<{ id: string; x: number; y: number }>,
  flows: WorkflowFlow[],
  boundaryPositions = { input: { x: -250, y: 220 } }
): WorkflowDefinition {
  const base = parseWorkflowDefinition(fixture)
  return {
    ...base,
    nodes: [
      ...nodes.map((node) => ({ ...base.nodes[0], ...node, name: node.id })),
      ...(flows.some((flow) => flow.target.kind === 'node' && flow.target.nodeId === 'result')
        ? [{ ...base.nodes[4], x: 1100, y: 220 }]
        : [])
    ],
    flows,
    boundaryPositions
  }
}

function routeLength(geometry: FlowGeometry) {
  return geometry.points.slice(1).reduce((length, point, index) => {
    const previous = geometry.points[index]
    return length + Math.abs(point.x - previous.x) + Math.abs(point.y - previous.y)
  }, 0)
}

function pathGeometry(points: CanvasPoint[], cornerRadius = 10): FlowGeometry {
  return {
    start: points[0],
    end: points.at(-1)!,
    points,
    cornerRadius,
    bridges: [],
    path: roundedFlowPath(points, cornerRadius),
    label: points[0]
  }
}

/** Ports touch their cards; every other part of an orthogonal route must stay outside cards. */
function expectClearRoute(
  graph: WorkflowDefinition,
  geometry: FlowGeometry,
  horizontalPorts = true
) {
  const cards = [...graph.nodes, graph.boundaryPositions.input]
  expect(geometry.points[0]).toEqual(geometry.start)
  expect(geometry.points.at(-1)).toEqual(geometry.end)
  expect(
    geometry.points.every((point) => Number.isFinite(point.x) && Number.isFinite(point.y))
  ).toBe(true)
  if (horizontalPorts) {
    expect(geometry.points[1].x).toBeGreaterThan(geometry.start.x)
    expect(geometry.points[1].y).toBe(geometry.start.y)
    expect(geometry.points.at(-2)!.x).toBeLessThan(geometry.end.x)
    expect(geometry.points.at(-2)!.y).toBe(geometry.end.y)
  }
  for (let index = 1; index < geometry.points.length; index++) {
    const a = geometry.points[index - 1],
      b = geometry.points[index]
    const horizontal = a.y === b.y,
      vertical = a.x === b.x
    expect(horizontal || vertical, `segment ${index} should remain orthogonal`).toBe(true)
    for (const card of cards) {
      const inset = 'kind' in card && card.kind !== 'agent' ? 2 : 0
      const dimensions = workflowNodeSize('kind' in card ? card : undefined)
      const crossesHorizontal =
        horizontal &&
        a.y > card.y + inset &&
        a.y < card.y + dimensions.height - inset &&
        Math.max(a.x, b.x) > card.x + inset &&
        Math.min(a.x, b.x) < card.x + dimensions.width - inset
      const crossesVertical =
        vertical &&
        a.x > card.x + inset &&
        a.x < card.x + dimensions.width - inset &&
        Math.max(a.y, b.y) > card.y + inset &&
        Math.min(a.y, b.y) < card.y + dimensions.height - inset
      expect(
        crossesHorizontal || crossesVertical,
        `segment ${index} crosses card at ${card.x},${card.y}`
      ).toBe(false)
    }
  }
}

describe('workflow canvas geometry', () => {
  it('keeps a parallel review workflow with two return loops below two crossings without moving nodes or anchors', () => {
    const graph = resolveWorkflowAnchors(reviewLoopGraph()),
      snapshot = structuredClone(graph)
    const before = new Map(graph.flows.map((f) => [f.id, flowGeometry(graph, f)]))
    const after = graphFlowGeometries(graph)
    const conflicts = (routes: Map<string, FlowGeometry | null>) => {
      const values = [...routes.values()].filter((g): g is FlowGeometry => !!g)
      let crossings = 0,
        overlap = 0
      for (let i = 0; i < values.length; i++)
        for (let j = i + 1; j < values.length; j++) {
          const cost = routeConflicts(values[i].points, values[j].points)
          crossings += cost.crossings
          overlap += cost.overlap
        }
      return { crossings, overlap }
    }
    const initial = conflicts(before),
      optimized = conflicts(after)
    expect(initial.crossings).toBeGreaterThan(0)
    expect(optimized.crossings).toBeLessThanOrEqual(initial.crossings)
    expect(optimized.crossings).toBeLessThanOrEqual(1)
    expect(optimized.overlap).toBe(0)
    for (const [id, geometry] of after) {
      expectClearRoute(graph, geometry!, false)
      expect(geometry!.start).toEqual(before.get(id)!.start)
      expect(geometry!.end).toEqual(before.get(id)!.end)
      expect(routeLength(geometry!)).toBeLessThanOrEqual(routeLength(before.get(id)!) * 1.6 + 120)
    }
    expect(graph).toEqual(snapshot)
    expect(graphFlowGeometries(graph)).toEqual(after)
    expect(graphFlowGeometries({ ...graph, flows: [...graph.flows].reverse() })).toEqual(after)
  })

  it('routes every pair of card sides around intervening obstacles without crossing cards', () => {
    for (const sourceSide of ['left', 'right', 'top', 'bottom'] as const) {
      for (const targetSide of ['left', 'right', 'top', 'bottom'] as const) {
        const graph = routingGraph(
          [
            { id: 'a', x: 100, y: 100 },
            { id: 'b', x: 800, y: 300 },
            { id: 'obstacle', x: 440, y: 180 }
          ],
          [connection('ab', 'a', 'b')]
        )
        const flow = graph.flows[0]
        flow.sourceAnchor = { side: sourceSide, offset: 0.3 }
        flow.targetAnchor = { side: targetSide, offset: 0.7 }
        const geometry = flowGeometry(graph, flow)!
        expectClearRoute(graph, geometry, false)
        const normals = { left: [-1, 0], right: [1, 0], top: [0, -1], bottom: [0, 1] }
        const direction = (a: CanvasPoint, b: CanvasPoint) => [
          Math.sign(b.x - a.x),
          Math.sign(b.y - a.y)
        ]
        expect(direction(geometry.start, geometry.points[1])).toEqual(normals[sourceSide])
        expect(direction(geometry.end, geometry.points.at(-2)!)).toEqual(normals[targetSide])
      }
    }
  })
  it('routes self loops from each side without cutting through the card', () => {
    for (const side of ['left', 'right', 'top', 'bottom'] as const) {
      const graph = routingGraph([{ id: 'a', x: 300, y: 200 }], [connection('loop', 'a', 'a')])
      graph.flows[0].sourceAnchor = { side, offset: 0.5 }
      graph.flows[0].targetAnchor = { side, offset: 0.5 }
      const geometry = flowGeometry(graph, graph.flows[0])!
      expect(geometry.points.length).toBeGreaterThan(5)
      expectClearRoute(graph, geometry, false)
    }
  })
  it('snaps regular cards to the clicked border and gates only to the tip or base', () => {
    const graph = parseWorkflowDefinition(fixture)
    const agent = { kind: 'node', nodeId: 'implement' } as const
    expect(anchorAtPoint(graph, agent, 'source', { x: 350, y: 181 })).toMatchObject({ side: 'top' })
    expect(anchorAtPoint(graph, agent, 'target', { x: 281, y: 205 })).toMatchObject({
      side: 'left'
    })
    const gate = graph.nodes.find((n) => n.kind === 'inputGate')!
    const ep = { kind: 'node', nodeId: gate.id } as const
    expect(anchorAtPoint(graph, ep, 'source', { x: gate.x, y: gate.y + 52 })).toEqual({
      side: 'right',
      offset: 0.5
    })
    expect(anchorAtPoint(graph, ep, 'target', { x: gate.x + 60, y: gate.y + 15 })).toEqual({
      side: 'left',
      offset: 0.25
    })
    expect(anchorPoint(graph, ep, 'source', { side: 'bottom', offset: 1 })).toEqual({
      x: gate.x + 62,
      y: gate.y + 28,
      side: 'right'
    })
  })

  it('keeps return paths outside node bodies and separates duplicate connections', () => {
    const graph = parseWorkflowDefinition(fixture)
    const flow = graph.flows.find((item) => item.id === 'revise')!
    const returnPath = flowGeometry(graph, flow)!
    expectClearRoute(graph, returnPath)
    const second = { ...flow, id: 'second-return' }
    graph.flows.push(second)
    const secondPath = flowGeometry(graph, second)!
    expect(secondPath.path).not.toBe(returnPath.path)
    expectClearRoute(graph, secondPath)
    const third = { ...flow, id: 'third-return' }
    graph.flows.push(third)
    const thirdPath = flowGeometry(graph, third)!
    expect(new Set([returnPath.path, secondPath.path, thirdPath.path]).size).toBe(3)
    expectClearRoute(graph, thirdPath)
    const self = {
      ...flow,
      id: 'self',
      source: { kind: 'node' as const, nodeId: graph.nodes[0].id },
      target: { kind: 'node' as const, nodeId: graph.nodes[0].id }
    }
    graph.flows.push(self)
    const selfPath = flowGeometry(graph, self)!
    expectClearRoute(graph, selfPath)
    const secondSelf = { ...self, id: 'second-self' }
    graph.flows.push(secondSelf)
    const secondSelfPath = flowGeometry(graph, secondSelf)!
    expect(secondSelfPath.path).not.toBe(selfPath.path)
    expectClearRoute(graph, secondSelfPath)
    const thirdSelf = { ...self, id: 'third-self' }
    graph.flows.push(thirdSelf)
    const thirdSelfPath = flowGeometry(graph, thirdSelf)!
    expect(new Set([selfPath.path, secondSelfPath.path, thirdSelfPath.path]).size).toBe(3)
    expectClearRoute(graph, thirdSelfPath)
  })

  it.each([220, 340])('keeps a 32px forward gap local when the target y is %s', (targetY) => {
    const flow = connection('narrow-gap', 'source', 'target')
    const graph = routingGraph(
      [
        { id: 'source', x: 80, y: 220 },
        { id: 'target', x: 80 + NODE_WIDTH + 32, y: targetY }
      ],
      [flow]
    )
    const geometry = flowGeometry(graph, flow)!
    expectClearRoute(graph, geometry)
    const directDistance =
      Math.abs(geometry.end.x - geometry.start.x) + Math.abs(geometry.end.y - geometry.start.y)
    expect(routeLength(geometry)).toBeCloseTo(directDistance)
  })

  it('uses local branches for staggered parallel workers that split and merge into review', () => {
    const graph = routingGraph(
      [
        { id: 'lead', x: 80, y: 260 },
        { id: 'frontend', x: 360, y: 80 },
        { id: 'backend', x: 400, y: 260 },
        { id: 'tests', x: 380, y: 440 },
        { id: 'review', x: 680, y: 260 }
      ],
      [
        connection('entry', null, 'lead'),
        ...['frontend', 'backend', 'tests'].flatMap((id) => [
          connection(`assign-${id}`, 'lead', id),
          connection(`review-${id}`, id, 'review')
        ]),
        connection('deliver', 'review', null)
      ],
      { input: { x: -220, y: 260 } }
    )
    for (const flow of graph.flows) {
      const geometry = flowGeometry(graph, flow)!
      expectClearRoute(graph, geometry)
      const directDistance =
        Math.abs(geometry.end.x - geometry.start.x) + Math.abs(geometry.end.y - geometry.start.y)
      expect(routeLength(geometry), flow.id).toBeCloseTo(directDistance)
    }
  })

  it('detours around a real obstacle without visiting unrelated distant cards', () => {
    const flow = connection('blocked', 'source', 'target')
    const graph = routingGraph(
      [
        { id: 'source', x: 80, y: 220 },
        { id: 'blocker', x: 430, y: 220 },
        { id: 'unrelated', x: 430, y: -500 },
        { id: 'target', x: 760, y: 220 }
      ],
      [flow]
    )
    const geometry = flowGeometry(graph, flow)!
    expectClearRoute(graph, geometry)
    expect(routeLength(geometry)).toBeLessThanOrEqual(
      geometry.end.x - geometry.start.x + 2 * (NODE_HEIGHT + 48)
    )
  })

  it.each(['input', 'output'] as const)(
    'avoids workers when the %s root is a connected endpoint',
    (side) => {
      const flow = connection(
        'boundary-edge',
        side === 'input' ? null : 'source',
        side === 'output' ? null : 'target'
      )
      const graph = routingGraph(
        [
          { id: 'blocker', x: 430, y: 220 },
          { id: side === 'input' ? 'target' : 'source', x: side === 'input' ? 760 : 80, y: 220 }
        ],
        [flow],
        {
          input: { x: side === 'input' ? 80 : -250, y: 220 }
        }
      )
      expectClearRoute(graph, flowGeometry(graph, flow)!)
    }
  )

  it.each(['input', 'output'] as const)(
    'treats an unconnected %s root as a card to avoid',
    (side) => {
      const flow = connection('root-blocker', 'source', 'target')
      const graph = routingGraph(
        [
          { id: 'source', x: 80, y: 220 },
          { id: 'target', x: 760, y: 220 }
        ],
        [flow]
      )
      graph.boundaryPositions[side] = { x: 430, y: 220 }
      expectClearRoute(graph, flowGeometry(graph, flow)!)
    }
  )

  it('keeps parallel forward edges distinct and outside every card', () => {
    const graph = routingGraph(
      [
        { id: 'source', x: 80, y: 220 },
        { id: 'target', x: 500, y: 220 }
      ],
      [0, 1, 2].map((index) => connection(`parallel-${index}`, 'source', 'target'))
    )
    const geometries = graph.flows.map((flow) => flowGeometry(graph, flow)!)
    expect(new Set(geometries.map((geometry) => geometry.path)).size).toBe(graph.flows.length)
    for (const geometry of geometries) expectClearRoute(graph, geometry)
  })

  it.each([220, 340])(
    'separates four parallel edges in a 32px gap when the target y is %s',
    (targetY) => {
      const graph = routingGraph(
        [
          { id: 'source', x: 80, y: 220 },
          { id: 'target', x: 80 + NODE_WIDTH + 32, y: targetY }
        ],
        [0, 1, 2, 3].map((index) => connection(`parallel-${index}`, 'source', 'target'))
      )
      const geometries = graph.flows.map((flow) => flowGeometry(graph, flow)!)
      expect(new Set(geometries.map((geometry) => geometry.path)).size).toBe(graph.flows.length)
      for (const geometry of geometries) expectClearRoute(graph, geometry)
    }
  )

  it('fits both root cards and loop routes without upscaling small graphs', () => {
    const graph = parseWorkflowDefinition(fixture)
    const bounds = graphBounds(graph)
    const entry = flowGeometry(
      graph,
      graph.flows.find((flow) => flow.id === 'entry')!
    )!
    const exit = flowGeometry(
      graph,
      graph.flows.find((flow) => flow.id === 'deliver')!
    )!
    expect(entry.start).toEqual({
      x: graph.boundaryPositions.input.x + NODE_WIDTH,
      y: graph.boundaryPositions.input.y + NODE_HEIGHT / 2
    })
    expect(exit.end).toEqual({
      x: graph.nodes.find((node) => node.id === 'result')!.x,
      y: graph.nodes.find((node) => node.id === 'result')!.y + NODE_HEIGHT / 2
    })
    expect(bounds.left).toBeLessThanOrEqual(graph.boundaryPositions.input.x)
    expect(bounds.right).toBeGreaterThanOrEqual(
      graph.nodes.find((node) => node.id === 'result')!.x + NODE_WIDTH
    )
    const origin = canvasOrigin(bounds)
    const viewport = fitViewport(bounds, 1800, 800, origin)
    expect(viewport.zoom).toBe(1)
    expect((bounds.left + origin.x) * viewport.zoom - viewport.x).toBeGreaterThanOrEqual(0)
    expect((bounds.right + origin.x) * viewport.zoom - viewport.x).toBeLessThanOrEqual(1800)
    const small = fitViewport(bounds, 480, 400, origin)
    expect(small.zoom).toBeGreaterThanOrEqual(0.25)
    expect(small.zoom).toBeLessThan(1)
    expect((bounds.bottom + origin.y) * small.zoom - small.y).toBeLessThanOrEqual(400)
    expect(bounds.bottom).toBeGreaterThanOrEqual(graph.nodes[0].y + NODE_HEIGHT)
  })

  it('routes all boundary connections to shared root cards and follows their positions', () => {
    const graph = parseWorkflowDefinition(fixture)
    const entry = graph.flows.find((flow) => flow.id === 'entry')!
    const exit = graph.flows.find((flow) => flow.id === 'deliver')!
    const anotherEntry = { ...entry, id: 'another-entry' }
    graph.flows.push(anotherEntry)
    const firstPath = flowGeometry(graph, entry)!
    const anotherPath = flowGeometry(graph, anotherEntry)!
    expect(anotherPath.start).toEqual(firstPath.start)
    expect(anotherPath.path).not.toBe(firstPath.path)

    graph.boundaryPositions.input = { x: -180, y: 300 }
    Object.assign(
      graph.nodes.find((node) => node.id === 'result')!,
      { x: 1400, y: 440 }
    )
    expect(flowGeometry(graph, entry)!.start).toEqual({
      x: -180 + NODE_WIDTH,
      y: 300 + NODE_HEIGHT / 2
    })
    expect(flowGeometry(graph, anotherEntry)!.start).toEqual(flowGeometry(graph, entry)!.start)
    expect(flowGeometry(graph, entry)!.end).toEqual(firstPath.end)
    expect(flowGeometry(graph, exit)!.end).toEqual({ x: 1400, y: 440 + NODE_HEIGHT / 2 })
    expect(graph.flows.find((flow) => flow.id === 'entry')!.source).toEqual({ kind: 'boundary' })
  })

  it('includes both root cards in an empty workflow and keeps worker IDs distinct from boundaries', () => {
    const graph = parseWorkflowDefinition(fixture)
    const empty = { ...graph, nodes: [], flows: [] }
    const bounds = graphBounds(empty)
    expect(bounds.left).toBeLessThanOrEqual(empty.boundaryPositions.input.x)
    expect(bounds.right).toBeGreaterThanOrEqual(empty.boundaryPositions.input.x + NODE_WIDTH)
    const origin = canvasOrigin(bounds)
    const viewport = fitViewport(bounds, 800, 600, origin)
    expect((bounds.left + origin.x) * viewport.zoom - viewport.x).toBeGreaterThanOrEqual(0)
    expect((bounds.right + origin.x) * viewport.zoom - viewport.x).toBeLessThanOrEqual(800)

    const node = { ...graph.nodes[0], id: 'boundary' }
    const flow = { ...graph.flows[0], target: { kind: 'node' as const, nodeId: node.id } }
    const geometry = flowGeometry({ ...graph, nodes: [node], flows: [flow] }, flow)!
    expect(geometry.start.x).toBe(graph.boundaryPositions.input.x + NODE_WIDTH)
    expect(geometry.end.x).toBe(node.x)
  })

  it('preserves the world coordinate beneath a zoom anchor and respects schema limits', () => {
    const before = { x: 420, y: 360, zoom: 1 }
    const anchor = { x: 160, y: 100 }
    const after = zoomViewport(before, 1.5, anchor)
    expect((after.x + anchor.x) / after.zoom).toBeCloseTo((before.x + anchor.x) / before.zoom)
    expect((after.y + anchor.y) / after.zoom).toBeCloseTo((before.y + anchor.y) / before.zoom)
    expect(zoomViewport(before, 10, anchor).zoom).toBe(2)
    expect(zoomViewport(before, 0, anchor).zoom).toBe(0.25)
    expect(zoomViewport({ x: 100000, y: 100000, zoom: 1 }, 2, anchor).x).toBeLessThanOrEqual(100000)
  })

  it.each([false, true])(
    'bridges a true crossing to the right on an upward=%s vertical route',
    (upward) => {
      const graph = routingGraph(
        [
          { id: 'vertical-source', x: 0, y: upward ? 400 : 0 },
          { id: 'vertical-target', x: 700, y: upward ? 0 : 400 },
          { id: 'horizontal-source', x: 0, y: 200 },
          { id: 'horizontal-target', x: 700, y: 200 }
        ],
        [
          connection('vertical', 'vertical-source', 'vertical-target'),
          connection('horizontal', 'horizontal-source', 'horizontal-target')
        ],
        { input: { x: -400, y: 800 } }
      )
      const geometries = addFlowCrossingBridges(
        new Map(graph.flows.map((flow) => [flow.id, flowGeometry(graph, flow)]))
      )
      const vertical = geometries.get('vertical')!
      const horizontal = geometries.get('horizontal')!
      expect(vertical.bridges).toHaveLength(1)
      const bridge = vertical.bridges[0]
      expect(bridge.point).toEqual({ x: (NODE_WIDTH + 700) / 2, y: 200 + NODE_HEIGHT / 2 })
      expect(bridge.radius).toBeGreaterThan(0)
      expect(bridge.radius).toBeLessThanOrEqual(12)
      expect(vertical.path).toContain(' A ')
      expect(bridge.path).toContain(' A ')
      expect(horizontal.bridges).toEqual([])
      expect(horizontal.path).not.toContain(' A ')
      const arc = bridge.path.match(
        /A\s+([\d.]+)[,\s]+([\d.]+)[,\s]+0[,\s]+0[,\s]+([01])[,\s]+([-\d.]+)[,\s]+([-\d.]+)/
      )!
      expect(arc).not.toBeNull()
      // With SVG's downward y axis, downward arcs sweep clockwise to pass on the right.
      expect(Number(arc[3])).toBe(upward ? 0 : 1)
      expect(Number(arc[4])).toBe(bridge.point.x)
      expect(Number(arc[5])).toBe(bridge.point.y + (upward ? -bridge.radius : bridge.radius))
      expect(vertical.start).toEqual(flowGeometry(graph, graph.flows[0])!.start)
      expect(vertical.end).toEqual(flowGeometry(graph, graph.flows[0])!.end)

      const reordered = addFlowCrossingBridges(
        new Map([...graph.flows].reverse().map((flow) => [flow.id, flowGeometry(graph, flow)]))
      )
      expect(reordered.get('vertical')!.bridges).toEqual(vertical.bridges)
      expect(reordered.get('horizontal')!.bridges).toEqual([])
    }
  )

  it.each(['split', 'merge'] as const)(
    'does not bridge shared trunks or junctions in a %s',
    (direction) => {
      const graph = routingGraph(
        direction === 'split'
          ? [
              { id: 'center', x: 0, y: 200 },
              { id: 'upper', x: 700, y: 0 },
              { id: 'lower', x: 700, y: 400 }
            ]
          : [
              { id: 'upper', x: 0, y: 0 },
              { id: 'lower', x: 0, y: 400 },
              { id: 'center', x: 700, y: 200 }
            ],
        ['upper', 'lower'].map((id) =>
          direction === 'split' ? connection(id, 'center', id) : connection(id, id, 'center')
        ),
        { input: { x: -400, y: 800 } }
      )
      for (const geometry of graphFlowGeometries(graph).values()) {
        expect(geometry!.bridges).toEqual([])
        expect(geometry!.path).not.toContain(' A ')
      }
    }
  )

  it('adds only one bridge when two shared horizontal trunks cover the same crossing', () => {
    const graph = routingGraph(
      [
        { id: 'source', x: 0, y: 200 },
        { id: 'upper', x: 900, y: 0 },
        { id: 'lower', x: 900, y: 400 },
        { id: 'vertical-source', x: 0, y: 0 },
        { id: 'vertical-target', x: 616, y: 400 }
      ],
      [
        connection('upper', 'source', 'upper'),
        connection('lower', 'source', 'lower'),
        connection('vertical', 'vertical-source', 'vertical-target')
      ],
      { input: { x: -400, y: 800 } }
    )
    const geometries = addFlowCrossingBridges(
      new Map(graph.flows.map((flow) => [flow.id, flowGeometry(graph, flow)]))
    )
    const vertical = geometries.get('vertical')!
    expect(vertical.bridges).toHaveLength(1)
    expect(vertical.bridges[0].point).toEqual({ x: 400, y: 200 + NODE_HEIGHT / 2 })
    for (const id of ['upper', 'lower']) {
      const horizontal = geometries.get(id)!
      expect(
        horizontal.points.some((point, index, points) => {
          const next = points[index + 1]
          return (
            next &&
            point.y === 200 + NODE_HEIGHT / 2 &&
            next.y === 200 + NODE_HEIGHT / 2 &&
            Math.min(point.x, next.x) < 400 &&
            Math.max(point.x, next.x) > 400
          )
        })
      ).toBe(true)
    }
  })

  it.each([6, 94])('does not bridge an apparent crossing inside a rounded corner at y=%s', (y) => {
    const vertical = pathGeometry([
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 100, y: 100 },
      { x: 200, y: 100 }
    ])
    const horizontal = pathGeometry([
      { x: 0, y },
      { x: 200, y }
    ])
    const result = addFlowCrossingBridges(
      new Map([
        ['vertical', vertical],
        ['horizontal', horizontal]
      ])
    )
    expect(result.get('vertical')!.bridges).toEqual([])
    expect(result.get('horizontal')!.bridges).toEqual([])
    expect(result.get('vertical')!.path).toBe(vertical.path)
  })

  it('does not bridge a crossing trimmed off the end of a rounded horizontal segment', () => {
    const result = addFlowCrossingBridges(
      new Map([
        [
          'vertical',
          pathGeometry([
            { x: 94, y: 0 },
            { x: 94, y: 100 }
          ])
        ],
        [
          'horizontal',
          pathGeometry([
            { x: 0, y: 50 },
            { x: 100, y: 50 },
            { x: 100, y: 150 }
          ])
        ]
      ])
    )
    expect(result.get('vertical')!.bridges).toEqual([])
  })

  it('ignores T-junction endpoint touches and collinear overlaps', () => {
    const result = addFlowCrossingBridges(
      new Map([
        [
          'vertical',
          pathGeometry([
            { x: 100, y: 0 },
            { x: 100, y: 100 }
          ])
        ],
        [
          'vertical-overlap',
          pathGeometry([
            { x: 100, y: 20 },
            { x: 100, y: 80 }
          ])
        ],
        [
          'touching-horizontal',
          pathGeometry([
            { x: 0, y: 50 },
            { x: 100, y: 50 }
          ])
        ],
        [
          'horizontal-overlap',
          pathGeometry([
            { x: 20, y: 50 },
            { x: 80, y: 50 }
          ])
        ]
      ])
    )
    for (const geometry of result.values()) expect(geometry!.bridges).toEqual([])
  })

  it('groups nearby crossings into one bridge without overlapping arcs', () => {
    const result = addFlowCrossingBridges(
      new Map([
        [
          'vertical',
          pathGeometry([
            { x: 100, y: 0 },
            { x: 100, y: 100 }
          ])
        ],
        [
          'horizontal-a',
          pathGeometry([
            { x: 0, y: 48 },
            { x: 200, y: 48 }
          ])
        ],
        [
          'horizontal-b',
          pathGeometry([
            { x: 0, y: 56 },
            { x: 200, y: 56 }
          ])
        ]
      ])
    )
    const vertical = result.get('vertical')!
    expect(vertical.bridges).toHaveLength(1)
    const bridge = vertical.bridges[0]
    expect(bridge.point.y - bridge.halfHeight).toBeLessThan(48)
    expect(bridge.point.y + bridge.halfHeight).toBeGreaterThan(56)
    expect(vertical.path.match(/ A /g)).toHaveLength(1)
  })

  it('keeps crossing bridges outside cards, using the left side or omitting a blocked bridge', () => {
    const geometries = new Map([
      [
        'vertical',
        pathGeometry([
          { x: 100, y: 0 },
          { x: 100, y: 100 }
        ])
      ],
      [
        'horizontal',
        pathGeometry([
          { x: 0, y: 50 },
          { x: 200, y: 50 }
        ])
      ]
    ])
    const rightCard = { left: 102, right: 140, top: 30, bottom: 70 }
    const leftCard = { left: 60, right: 98, top: 30, bottom: 70 }
    const shifted = addFlowCrossingBridges(geometries, [rightCard]).get('vertical')!
    expect(shifted.bridges).toHaveLength(1)
    expect(shifted.bridges[0].direction).toBe(-1)
    expect(
      addFlowCrossingBridges(geometries, [leftCard, rightCard]).get('vertical')!.bridges
    ).toEqual([])
  })
})
