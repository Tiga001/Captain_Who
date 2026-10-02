import { AccountAvatar } from '../auth/AccountAvatar'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { AgentAvatar } from '../agentCollaboration/AgentAvatar'
import type { WorkflowDefinition } from '@mycopilot/protocol'
import { Inbox, Maximize2, Minus, Plus, WandSparkles } from 'lucide-react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { NODE_WIDTH, NODE_HEIGHT, WORKFLOW_DRAG_TYPE } from './workflowAuthoring'
import { fitViewport, canvasOrigin, graphBounds, zoomViewport } from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import {
  workflowNodeModelLabel,
  workflowNodeLabel,
  type WorkflowModelDisplay
} from './workflowModelPresentation'
import './workflowCanvas.css'

export type WorkflowSelection = { kind: 'node'; id: string } | null
export interface WorkflowCanvasChangeOptions {
  group?: string
  transient?: boolean
}
interface Props {
  graph: WorkflowDefinition
  models: readonly WorkflowModelDisplay[]
  selection: WorkflowSelection
  text: WorkflowText
  onChange: (
    update: (graph: WorkflowDefinition) => WorkflowDefinition,
    options?: WorkflowCanvasChangeOptions
  ) => void
  onSelect: (selection: WorkflowSelection) => void
  onConfigureNode: (id: string) => void
  onAddNode: (x: number, y: number, nodeType?: string) => void
}
export function WorkflowCanvas({
  graph,
  models,
  selection,
  text,
  onChange,
  onSelect,
  onConfigureNode,
  onAddNode
}: Props) {
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || text('currentUser')
  const scrollRef = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ width: 900, height: 550 })
  const [panning, setPanning] = useState(false)
  const drag = useRef<{
    id: string
    pointer: number
    x: number
    y: number
    left: number
    top: number
    group: string
  } | null>(null)
  const pan = useRef<{ pointer: number; x: number; y: number; left: number; top: number } | null>(
    null
  )
  const bounds = graphBounds(graph),
    origin = canvasOrigin(bounds),
    zoom = graph.viewport.zoom
  const latest = useRef({ graph, onChange, origin })
  useLayoutEffect(() => {
    latest.current = { graph, onChange, origin }
  })
  useLayoutEffect(() => {
    const element = scrollRef.current
    if (!element) return
    const measure = () => setSize({ width: element.clientWidth, height: element.clientHeight })
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  useLayoutEffect(() => {
    const element = scrollRef.current
    if (!element) return
    element.scrollLeft = graph.viewport.x
    element.scrollTop = graph.viewport.y
  }, [graph.viewport.x, graph.viewport.y])
  useEffect(() => {
    const element = scrollRef.current
    if (!element) return
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      const { graph, onChange } = latest.current
      const rect = element.getBoundingClientRect()
      const delta =
        event.deltaY *
        (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.clientHeight : 1)
      onChange(
        (current) => ({
          ...current,
          viewport: zoomViewport(graph.viewport, graph.viewport.zoom * Math.exp(-delta * 0.002), {
            x: event.clientX - rect.left,
            y: event.clientY - rect.top
          })
        }),
        { transient: true }
      )
    }
    element.addEventListener('wheel', wheel, { passive: false })
    return () => element.removeEventListener('wheel', wheel)
  }, [])
  const width = Math.max(900, bounds.right + origin.x + 240, (graph.viewport.x + size.width) / zoom)
  const height = Math.max(
    650,
    bounds.bottom + origin.y + 180,
    (graph.viewport.y + size.height) / zoom
  )
  const changeZoom = (next: number) =>
    onChange(
      (current) => ({
        ...current,
        viewport: zoomViewport(current.viewport, next, { x: size.width / 2, y: size.height / 2 })
      }),
      { transient: true }
    )
  const reset = () =>
    onChange(
      (current) => ({ ...current, viewport: fitViewport(bounds, size.width, size.height, origin) }),
      { transient: true }
    )
  return (
    <div className="workflow-canvas-shell">
      <div
        ref={scrollRef}
        tabIndex={0}
        className={`workflow-canvas${panning ? ' is-panning' : ''}`}
        aria-label={text('structure')}
        onScroll={(event) => {
          const element = event.currentTarget
          if (element.scrollLeft !== graph.viewport.x || element.scrollTop !== graph.viewport.y)
            onChange(
              (current) => ({
                ...current,
                viewport: { ...current.viewport, x: element.scrollLeft, y: element.scrollTop }
              }),
              { transient: true }
            )
        }}
        onPointerDown={(event) => {
          if (event.button !== 0 && event.button !== 1) return
          if ((event.target as Element).closest('[data-workflow-node-id]')) return
          event.preventDefault()
          onSelect(null)
          pan.current = {
            pointer: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            left: graph.viewport.x,
            top: graph.viewport.y
          }
          event.currentTarget.setPointerCapture(event.pointerId)
          setPanning(true)
        }}
        onPointerMove={(event) => {
          const item = drag.current
          if (item && item.pointer === event.pointerId) {
            const x = Math.max(24, Math.min(99000, item.left + (event.clientX - item.x) / zoom)),
              y = Math.max(40, Math.min(99000, item.top + (event.clientY - item.y) / zoom))
            onChange(
              (current) => ({
                ...current,
                nodes: current.nodes.map((n) => (n.id === item.id ? { ...n, x, y } : n))
              }),
              { group: item.group }
            )
            return
          }
          const start = pan.current
          if (start && start.pointer === event.pointerId)
            onChange(
              (current) => ({
                ...current,
                viewport: {
                  ...current.viewport,
                  x: Math.max(0, start.left + start.x - event.clientX),
                  y: Math.max(0, start.top + start.y - event.clientY)
                }
              }),
              { transient: true }
            )
        }}
        onPointerUp={(event) => {
          drag.current = null
          pan.current = null
          setPanning(false)
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            event.currentTarget.releasePointerCapture(event.pointerId)
        }}
        onPointerCancel={() => {
          drag.current = null
          pan.current = null
          setPanning(false)
        }}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes(WORKFLOW_DRAG_TYPE)) {
            event.preventDefault()
            event.dataTransfer.dropEffect = 'copy'
          }
        }}
        onDrop={(event) => {
          const value = event.dataTransfer.getData(WORKFLOW_DRAG_TYPE)
          if (value !== 'blank' && value !== 'node:user') return
          event.preventDefault()
          const rect = event.currentTarget.getBoundingClientRect()
          onAddNode(
            (event.clientX - rect.left + graph.viewport.x) / zoom - origin.x - NODE_WIDTH / 2,
            (event.clientY - rect.top + graph.viewport.y) / zoom - origin.y - NODE_HEIGHT / 2,
            value
          )
        }}
      >
        <div
          className="workflow-canvas__extent"
          style={{ width: width * zoom, height: height * zoom }}
        >
          <div
            className="workflow-canvas__stage"
            style={{ width, height, transform: `scale(${zoom})` }}
          >
            {graph.nodes.map((node) => (
              <div
                key={node.id}
                data-workflow-node-id={node.id}
                className={`workflow-node${selection?.id === node.id ? ' is-selected' : ''}`}
                style={{
                  left: node.x + origin.x,
                  top: node.y + origin.y,
                  width: NODE_WIDTH,
                  height: NODE_HEIGHT
                }}
                role="group"
                tabIndex={0}
                aria-label={`${text('node')} ${workflowNodeLabel(node, text, userName)}`}
                onClick={() => onSelect({ kind: 'node', id: node.id })}
                onDoubleClick={() => onConfigureNode(node.id)}
                onPointerDown={(event) => {
                  if (event.button !== 0) return
                  event.stopPropagation()
                  event.preventDefault()
                  event.currentTarget.focus()
                  onSelect({ kind: 'node', id: node.id })
                  drag.current = {
                    id: node.id,
                    pointer: event.pointerId,
                    x: event.clientX,
                    y: event.clientY,
                    left: node.x,
                    top: node.y,
                    group: `node-move-${crypto.randomUUID()}`
                  }
                  event.currentTarget.setPointerCapture(event.pointerId)
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    onConfigureNode(node.id)
                    return
                  }
                  const d = (
                    {
                      ArrowLeft: [-1, 0],
                      ArrowRight: [1, 0],
                      ArrowUp: [0, -1],
                      ArrowDown: [0, 1]
                    } as Record<string, number[]>
                  )[event.key]
                  if (!d) return
                  event.preventDefault()
                  const step = event.shiftKey ? 20 : 4
                  onChange(
                    (current) => ({
                      ...current,
                      nodes: current.nodes.map((n) =>
                        n.id === node.id ? { ...n, x: n.x + d[0] * step, y: n.y + d[1] * step } : n
                      )
                    }),
                    { group: `node-key-${node.id}` }
                  )
                }}
              >
                <span className="workflow-node__mailbox" aria-hidden="true">
                  <Inbox size={19} />
                </span>
                {node.kind === 'user' ? (
                  <span className="workflow-node__avatar workflow-user-avatar">
                    <AccountAvatar
                      src={profile?.avatarDataUrl}
                      localAvatarSeed={profile?.localAccount?.avatarSeed}
                    />
                  </span>
                ) : (
                  <AgentAvatar agentId={node.id} className="workflow-node__avatar" />
                )}
                <div className="workflow-node__copy workflow-node__copy--configurable">
                  <strong>{workflowNodeLabel(node, text, userName)}</strong>
                  <span title={workflowNodeModelLabel(node, models, text)}>
                    {workflowNodeModelLabel(node, models, text)}
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="workflow-canvas-actions">
        <button
          type="button"
          className="workflow-canvas-optimize"
          aria-label={text('optimizeLayout')}
          title={text('optimizeLayout')}
          disabled={!graph.nodes.length}
          onClick={() =>
            onChange((current) => ({
              ...current,
              nodes: current.nodes.map((node, index) => ({
                ...node,
                x: 80 + (index % 3) * (NODE_WIDTH + 64),
                y: 100 + Math.floor(index / 3) * (NODE_HEIGHT + 64)
              }))
            }))
          }
        >
          <WandSparkles size={18} />
        </button>
        <div className="workflow-canvas-controls" role="group" aria-label={text('resetView')}>
          <button type="button" aria-label={text('zoomOut')} onClick={() => changeZoom(zoom - 0.1)}>
            <Minus size={18} />
          </button>
          <button
            type="button"
            className="workflow-canvas-controls__percentage"
            onClick={() => changeZoom(1)}
          >
            {Math.round(zoom * 100)}%
          </button>
          <button type="button" aria-label={text('zoomIn')} onClick={() => changeZoom(zoom + 0.1)}>
            <Plus size={18} />
          </button>
          <span className="workflow-canvas-controls__separator" />
          <button type="button" aria-label={text('resetView')} onClick={reset}>
            <Maximize2 size={18} />
          </button>
        </div>
      </div>
    </div>
  )
}
