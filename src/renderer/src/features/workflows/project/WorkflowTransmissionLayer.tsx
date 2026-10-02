import { useId, useLayoutEffect, useRef } from 'react'
import type { WorkflowNode, WorkflowRuntimeEvent } from '@mycopilot/protocol'
import { workflowNodeSize } from '../workflowAuthoring'
import type { CanvasPoint } from '../workflowCanvasGeometry'

function TransmissionPulse({ path }: { path: string }) {
  const animation = useRef<SVGAnimateMotionElement>(null)
  useLayoutEffect(() => {
    // The parent SVG can have been open for hours. Start relative to this event's
    // mount, not the SVG document's zero timestamp; re-renders must not restart it.
    animation.current?.beginElement()
  }, [])
  return (
    <circle r="4" className="workflow-monitor__transmission">
      <animateMotion ref={animation} path={path} begin="indefinite" dur="2s" fill="freeze" />
    </circle>
  )
}

/** Transient presentation: paths are computed from members, never saved as workflow routes. */
export function transmissionPath(source: WorkflowNode, target: WorkflowNode): string {
  const aSize = workflowNodeSize(source),
    bSize = workflowNodeSize(target)
  const a = { x: source.x + aSize.width / 2, y: source.y + aSize.height / 2 }
  const b = { x: target.x + bSize.width / 2, y: target.y + bSize.height / 2 }
  if (source.id === target.id) return ''
  const dx = b.x - a.x,
    dy = b.y - a.y
  const horizontal = Math.abs(dx) / aSize.width >= Math.abs(dy) / aSize.height
  if (horizontal) {
    const sign = dx >= 0 ? 1 : -1
    const x1 = a.x + (sign * aSize.width) / 2,
      x2 = b.x - (sign * bSize.width) / 2
    const bend = Math.max(40, Math.abs(x2 - x1) * 0.45)
    return `M ${x1} ${a.y} C ${x1 + sign * bend} ${a.y}, ${x2 - sign * bend} ${b.y}, ${x2} ${b.y}`
  }
  const sign = dy >= 0 ? 1 : -1,
    y1 = a.y + (sign * aSize.height) / 2,
    y2 = b.y - (sign * bSize.height) / 2,
    bend = Math.max(40, Math.abs(y2 - y1) * 0.45)
  return `M ${a.x} ${y1} C ${a.x} ${y1 + sign * bend}, ${b.x} ${y2 - sign * bend}, ${b.x} ${y2}`
}
export function WorkflowTransmissionLayer({
  nodes,
  events,
  origin,
  width,
  height
}: {
  nodes: readonly WorkflowNode[]
  events: readonly WorkflowRuntimeEvent[]
  origin: CanvasPoint
  width: number
  height: number
}) {
  const marker = useId().replaceAll(':', '')
  const byId = new Map(nodes.map((n) => [n.id, n]))
  return (
    <svg
      className="workflow-canvas__edges workflow-mail-transmissions"
      width={width}
      height={height}
      aria-hidden="true"
    >
      <defs>
        <marker id={marker} markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto">
          <path d="M 0 0 L 7 3.5 L 0 7 z" fill="currentColor" />
        </marker>
      </defs>
      <g transform={`translate(${origin.x} ${origin.y})`}>
        {events.map((event) => {
          if (event.kind !== 'sent' && event.kind !== 'recalled') return null
          const source = event.sourceNodeId ? byId.get(event.sourceNodeId) : undefined,
            target = event.targetNodeId ? byId.get(event.targetNodeId) : undefined
          if (!source || !target) return null
          const path = transmissionPath(source, target)
          if (!path) return null
          return (
            <g
              key={event.sequence}
              className={`workflow-mail-transmission is-${event.kind}`}
              data-transmission-sequence={event.sequence}
              data-source-node={source.id}
              data-target-node={target.id}
            >
              <path
                className="workflow-mail-transmission__line"
                d={path}
                markerEnd={`url(#${marker})`}
              />
              <TransmissionPulse path={path} />
            </g>
          )
        })}
      </g>
    </svg>
  )
}
