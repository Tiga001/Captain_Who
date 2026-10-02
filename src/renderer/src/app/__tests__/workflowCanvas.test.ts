import { describe, expect, it } from 'vitest'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import {
  canvasOrigin,
  fitViewport,
  graphBounds,
  zoomViewport
} from '../../features/workflows/workflowCanvasGeometry'

describe('mail network canvas geometry', () => {
  it('fits all independent members including negative coordinates, without synthetic roots', () => {
    const graph = createWorkflow()
    graph.nodes = [createWorkflowNode('A', -350, -100), createWorkflowNode('B', 900, 800)]
    const bounds = graphBounds(graph)
    expect(bounds).toEqual({ left: -359, top: -105, right: 1121, bottom: 857 })
    const origin = canvasOrigin(bounds)
    const view = fitViewport(bounds, 900, 700, origin)
    expect(view.zoom).toBeGreaterThanOrEqual(0.25)
    expect(view.zoom).toBeLessThan(1)
    expect(view.x).toBeGreaterThanOrEqual(0)
    expect(view.y).toBeGreaterThanOrEqual(0)
    expect(graph.nodes.map((n) => [n.x, n.y])).toEqual([
      [-350, -100],
      [900, 800]
    ])
  })
  it('uses finite empty bounds without inventing user nodes', () => {
    const graph = createWorkflow()
    expect(graphBounds(graph)).toEqual({ left: 0, top: 0, right: 320, bottom: 160 })
    expect(fitViewport(graphBounds(graph), 900, 700, canvasOrigin(graphBounds(graph))).zoom).toBe(1)
    expect(graph.nodes).toEqual([])
  })
  it('preserves the world coordinate under the zoom anchor and clamps zoom', () => {
    const old = { x: 400, y: 300, zoom: 1 },
      anchor = { x: 200, y: 150 }
    const next = zoomViewport(old, 1.5, anchor)
    expect((next.x + anchor.x) / next.zoom).toBe((old.x + anchor.x) / old.zoom)
    expect((next.y + anchor.y) / next.zoom).toBe((old.y + anchor.y) / old.zoom)
    expect(zoomViewport(old, 100, anchor).zoom).toBe(2)
    expect(zoomViewport(old, 0, anchor).zoom).toBe(0.25)
  })
})
