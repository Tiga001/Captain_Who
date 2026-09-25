import { describe, expect, it } from 'vitest'
import {
  parseWorkflowDefinition,
  type WorkflowDefinition,
  type WorkflowFlow
} from '@mycopilot/protocol'
import {
  createWorkflow,
  createWorkflowGate,
  createWorkflowNode,
  workflowNodeSize
} from '../../features/workflows/workflowAuthoring'
import {
  optimizeWorkflowLayout,
  resolveWorkflowAnchors
} from '../../features/workflows/workflowAnchorLayout'
import { graphFlowLayout } from '../../features/workflows/workflowCanvasGeometry'
import { routeConflicts } from '../../features/workflows/workflowRouteOptimization'
import {
  createWorkflowHistory,
  workflowHistoryReducer
} from '../../features/workflows/workflowHistory'
import { reviewLoopGraph } from './workflowRoutingFixtures'

function node(id: string, x: number, y: number) {
  return { ...createWorkflowNode(id, x, y), id }
}
function flow(id: string, from: string, to: string): WorkflowFlow {
  return {
    id,
    name: id,
    source: { kind: 'node', nodeId: from },
    target: { kind: 'node', nodeId: to }
  }
}
function gateGraph(output = false): WorkflowDefinition {
  const gate = {
    ...createWorkflowGate(output ? 'outputGate' : 'inputGate', output ? 40 : 400, 180),
    id: 'gate'
  }
  return {
    ...createWorkflow(),
    nodes: [
      node('top', output ? 360 : 40, 40),
      node('middle', output ? 360 : 40, 180),
      node('bottom', output ? 360 : 40, 320),
      gate
    ],
    // Deliberately create the bottom connection before the middle one.
    flows: ['top', 'bottom', 'middle'].map((id) =>
      output ? flow(id, 'gate', id) : flow(id, id, 'gate')
    ),
    boundaryPositions: { input: { x: -300, y: 200 }, output: { x: 1000, y: 200 } }
  }
}
function noOverlaps(graph: WorkflowDefinition) {
  const boxes = [
    ...graph.nodes.map((node) => ({ ...node, ...workflowNodeSize(node) })),
    ...Object.values(graph.boundaryPositions).map((p) => ({ ...p, ...workflowNodeSize() }))
  ]
  for (let i = 0; i < boxes.length; i++)
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i],
        b = boxes[j]
      expect(
        a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y
      ).toBe(false)
    }
}

describe('workflow automatic layout', () => {
  it.each([
    [400, 0, 'right', 'left'],
    [-400, 0, 'left', 'right'],
    [0, 250, 'bottom', 'top'],
    [0, -250, 'top', 'bottom']
  ] as const)('chooses facing sides for a neighbor at %s,%s', (x, y, source, target) => {
    const graph = {
      ...createWorkflow(),
      nodes: [node('a', 0, 0), node('b', x, y)],
      flows: [flow('S1', 'a', 'b')]
    }
    const resolved = resolveWorkflowAnchors(graph)
    expect(resolved.flows[0].sourceAnchor?.side).toBe(source)
    expect(resolved.flows[0].targetAnchor?.side).toBe(target)
    expect(graph.flows[0].sourceAnchor).toBeUndefined()
  })

  it.each([false, true])(
    'sorts gate base anchors spatially instead of by creation order (output=%s)',
    (output) => {
      const graph = gateGraph(output),
        key = output ? 'sourceAnchor' : 'targetAnchor'
      const { graph: resolved, geometries } = graphFlowLayout(graph)
      const offset = (id: string) => resolved.flows.find((f) => f.id === id)![key]!.offset
      expect(offset('top')).toBeLessThan(offset('middle'))
      expect(offset('middle')).toBeLessThan(offset('bottom'))
      const routes = [...geometries.values()]
      for (let i = 0; i < routes.length; i++)
        for (let j = i + 1; j < routes.length; j++)
          expect(routeConflicts(routes[i]!.points, routes[j]!.points).crossings).toBe(0)
      const reversed = graphFlowLayout({ ...graph, flows: [...graph.flows].reverse() })
      expect(reversed.geometries).toEqual(geometries)
    }
  )

  it('keeps a manually dragged endpoint fixed and spaces automatic ports around it', () => {
    const graph = gateGraph()
    graph.flows[0].targetAnchor = { side: 'left', offset: 0.5 }
    graph.flows[0].sourceAnchor = { side: 'bottom', offset: 0.3 }
    const { graph: resolved } = graphFlowLayout(graph)
    expect(resolved.flows[0].targetAnchor).toEqual(graph.flows[0].targetAnchor)
    expect(resolved.flows[0].sourceAnchor).toEqual(graph.flows[0].sourceAnchor)
    const offsets = resolved.flows.map((f) => f.targetAnchor!.offset)
    expect(new Set(offsets).size).toBe(3)
    expect(graphFlowLayout(resolved).graph).toEqual(resolved)
  })

  it('arranges a parallel cyclic workflow without changing its connections, rules or node content', () => {
    const graph = reviewLoopGraph(),
      snapshot = structuredClone(graph)
    const next = optimizeWorkflowLayout(graph)
    noOverlaps(next)
    const routes = [...graphFlowLayout(next).geometries.values()]
    for (let i = 0; i < routes.length; i++)
      for (let j = i + 1; j < routes.length; j++)
        expect(routeConflicts(routes[i]!.points, routes[j]!.points).crossings).toBe(0)
    expect(next.nodes).toHaveLength(graph.nodes.length)
    expect(
      next.flows.map((f) => ({ ...f, sourceAnchor: undefined, targetAnchor: undefined }))
    ).toEqual(graph.flows.map((f) => ({ ...f, sourceAnchor: undefined, targetAnchor: undefined })))
    expect(next.nodes.map((n) => ({ ...n, x: 0, y: 0 }))).toEqual(
      graph.nodes.map((n) => ({ ...n, x: 0, y: 0 }))
    )
    expect(next.boundaryPositions.input.x).toBeLessThan(Math.min(...next.nodes.map((n) => n.x)))
    expect(next.boundaryPositions.output.x).toBeGreaterThan(Math.max(...next.nodes.map((n) => n.x)))
    const position = (id: string) => next.nodes.find((n) => n.id === id)!
    expect(position('a').x).toBe(position('b').x)
    expect(position('b').x).toBe(position('c').x)
    expect(position('merge').x).toBeLessThan(position('review').x)
    expect(position('review').x).toBeLessThan(position('output').x)
    expect(position('merge').y + 28).toBe(position('review').y + 26)
    expect(next.flows.every((f) => !f.sourceAnchor && !f.targetAnchor)).toBe(true)
    expect(parseWorkflowDefinition(JSON.parse(JSON.stringify(next)))).toEqual(next)
    expect(optimizeWorkflowLayout(next)).toEqual(next)
    expect(graph).toEqual(snapshot)
    const state = workflowHistoryReducer(createWorkflowHistory(graph), {
      type: 'change',
      update: optimizeWorkflowLayout,
      at: 1
    })
    expect(state.past).toHaveLength(1)
    expect(workflowHistoryReducer(state, { type: 'undo' }).present).toEqual(graph)
    expect(
      workflowHistoryReducer(workflowHistoryReducer(state, { type: 'undo' }), { type: 'redo' })
        .present
    ).toEqual(next)
  })

  it('reorders parallel columns to uncross independent branches', () => {
    const graph = {
      ...createWorkflow(),
      nodes: [node('a', 0, 0), node('b', 0, 100), node('c', 400, 0), node('d', 400, 100)],
      flows: [flow('ad', 'a', 'd'), flow('bc', 'b', 'c')]
    }
    const next = optimizeWorkflowLayout(graph)
    const a = next.nodes.find((n) => n.id === 'a')!,
      b = next.nodes.find((n) => n.id === 'b')!
    const c = next.nodes.find((n) => n.id === 'c')!,
      d = next.nodes.find((n) => n.id === 'd')!
    expect((a.y - b.y) * (d.y - c.y)).toBeGreaterThan(0)
    noOverlaps(next)
    expect(optimizeWorkflowLayout(next)).toEqual(next)
  })

  it('lays out disconnected drafts, orphan gates and empty graphs without overlaps or dropping nodes', () => {
    const graph = gateGraph()
    graph.nodes.push(
      { ...createWorkflowGate('outputGate', 400, 180), id: 'orphan' },
      node('isolated', 400, 180)
    )
    graph.flows = []
    const next = optimizeWorkflowLayout(graph)
    noOverlaps(next)
    expect(next.nodes.map((n) => n.id)).toEqual(graph.nodes.map((n) => n.id))
    noOverlaps(optimizeWorkflowLayout(createWorkflow()))
  })
})
