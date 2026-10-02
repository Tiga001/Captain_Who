import type { WorkflowDefinition } from '@mycopilot/protocol'
import { workflowNodeSize } from './workflowAuthoring'
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
export function graphBounds(
  graph: Pick<WorkflowDefinition, 'nodes' | 'departments'>
): CanvasBounds {
  const bounds = graph.nodes.map((n) => ({
    left: n.x - 9,
    top: n.y - 5,
    right: n.x + workflowNodeSize(n).width + 9,
    bottom: n.y + workflowNodeSize(n).height + 5
  }))
  bounds.push(
    ...(graph.departments ?? []).map((department) => ({
      left: department.x - 4,
      top: department.y - 4,
      right: department.x + department.width + 4,
      bottom: department.y + department.height + 4
    }))
  )
  return bounds.length
    ? {
        left: Math.min(...bounds.map((b) => b.left)),
        top: Math.min(...bounds.map((b) => b.top)),
        right: Math.max(...bounds.map((b) => b.right)),
        bottom: Math.max(...bounds.map((b) => b.bottom))
      }
    : { left: 0, top: 0, right: 320, bottom: 160 }
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
