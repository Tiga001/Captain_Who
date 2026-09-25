import type { AgentTemplate, WorkflowDefinition, WorkflowEndpoint } from '@mycopilot/protocol'
import { Maximize2, Minus, Plus, Settings } from 'lucide-react'
import {
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent
} from 'react'
import { NODE_HEIGHT, NODE_WIDTH, WORKFLOW_DRAG_TYPE } from './workflowAuthoring'
import {
  fitViewport,
  canvasOrigin,
  graphFlowGeometries,
  graphBounds,
  zoomViewport,
  type CanvasPoint
} from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import { workflowNodeModelLabel, type WorkflowModelDisplay } from './workflowModelPresentation'
import './workflowCanvas.css'

type CardSelection = { kind: 'node'; id: string } | { kind: 'boundary'; id: 'input' | 'output' }
export type WorkflowSelection = CardSelection | { kind: 'flow'; id: string } | null
export interface WorkflowCanvasChangeOptions {
  group?: string
  transient?: boolean
}
interface Props {
  graph: WorkflowDefinition
  templates: readonly AgentTemplate[]
  models: readonly WorkflowModelDisplay[]
  selection: WorkflowSelection
  pending: WorkflowEndpoint | null
  text: WorkflowText
  onChange: (
    update: (graph: WorkflowDefinition) => WorkflowDefinition,
    options?: WorkflowCanvasChangeOptions
  ) => void
  onSelect: (selection: WorkflowSelection) => void
  onConfigureNode: (id: string) => void
  onAddNode: (x: number, y: number, templateId?: string) => void
  onBegin: (endpoint: WorkflowEndpoint) => void
  onFinish: (endpoint: WorkflowEndpoint) => void
}

const isEditingText = (target: EventTarget | null) =>
  target instanceof Element &&
  !!target.closest('input, textarea, select, button, [role="button"], [contenteditable="true"]')
const boundedPosition = (position: number, minimum: number) =>
  Math.max(minimum, Math.min(99000, position))

function positionCard(
  graph: WorkflowDefinition,
  target: CardSelection,
  point: CanvasPoint
): WorkflowDefinition {
  return target.kind === 'node'
    ? {
        ...graph,
        nodes: graph.nodes.map((node) => (node.id === target.id ? { ...node, ...point } : node))
      }
    : { ...graph, boundaryPositions: { ...graph.boundaryPositions, [target.id]: point } }
}

export function WorkflowCanvas({
  graph,
  templates,
  models,
  selection,
  pending,
  text,
  onChange,
  onSelect,
  onConfigureNode,
  onAddNode,
  onBegin,
  onFinish
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const stageRef = useRef<HTMLDivElement>(null)
  const drag = useRef<{
    target: CardSelection
    pointerId: number
    x: number
    y: number
    startX: number
    startY: number
    zoom: number
    group: string
  } | null>(null)
  const connectionDrag = useRef<{ pointerId: number; x: number; y: number } | null>(null)
  const pan = useRef<{ pointerId: number; x: number; y: number; left: number; top: number } | null>(
    null
  )
  const space = useRef(false)
  const hovered = useRef(false)
  const [spaceHeld, setSpaceHeld] = useState(false)
  const [panning, setPanning] = useState(false)
  const [cursor, setCursor] = useState<CanvasPoint | null>(null)
  const [size, setSize] = useState({ width: 0, height: 0 })
  const initialFit = useRef<string | null>(null)
  const markerId = useId().replaceAll(':', '')
  const geometries = useMemo(
    () =>
      graphFlowGeometries({
        nodes: graph.nodes,
        flows: graph.flows,
        boundaryPositions: graph.boundaryPositions
      }),
    [graph.nodes, graph.flows, graph.boundaryPositions]
  )
  const bounds = useMemo(
    () =>
      graphBounds(
        { nodes: graph.nodes, flows: graph.flows, boundaryPositions: graph.boundaryPositions },
        geometries
      ),
    [graph.nodes, graph.flows, graph.boundaryPositions, geometries]
  )
  const origin = useMemo(() => canvasOrigin(bounds), [bounds])
  const previousOrigin = useRef(origin)
  const zoom = graph.viewport.zoom
  const width = Math.max(
    size.width / zoom + graph.viewport.x / zoom + 80,
    bounds.right + origin.x + 240,
    900
  )
  const height = Math.max(
    size.height / zoom + graph.viewport.y / zoom + 80,
    bounds.bottom + origin.y + 180,
    650
  )
  const callbacks = useRef({ onChange, graph, origin })
  useLayoutEffect(() => {
    callbacks.current = { onChange, graph, origin }
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
    if (initialFit.current === graph.id || !size.width || !size.height) return
    initialFit.current = graph.id
    onChange(
      (current) => ({
        ...current,
        viewport: fitViewport(bounds, size.width, size.height, origin)
      }),
      { transient: true }
    )
  }, [graph.id, bounds, size, origin, onChange])

  useLayoutEffect(() => {
    const previous = previousOrigin.current
    previousOrigin.current = origin
    if (previous.x !== origin.x || previous.y !== origin.y) {
      onChange(
        (current) => ({
          ...current,
          viewport: {
            ...current.viewport,
            x: Math.min(
              100000,
              Math.max(0, current.viewport.x + (origin.x - previous.x) * current.viewport.zoom)
            ),
            y: Math.min(
              100000,
              Math.max(0, current.viewport.y + (origin.y - previous.y) * current.viewport.zoom)
            )
          }
        }),
        { transient: true }
      )
    }
  }, [origin, onChange])

  useLayoutEffect(() => {
    const element = scrollRef.current
    if (element && size.width && size.height) {
      element.scrollLeft = graph.viewport.x
      element.scrollTop = graph.viewport.y
    }
    // Hidden tab panels have no scroll range. Reapply even when fitting the newly visible
    // canvas produces the same persisted viewport, so a previous no-op scroll is repaired.
  }, [graph.viewport.x, graph.viewport.y, graph.viewport.zoom, size.width, size.height])

  useLayoutEffect(() => {
    const element = scrollRef.current
    if (
      !element ||
      !size.width ||
      !size.height ||
      !selection ||
      selection.kind === 'flow' ||
      drag.current ||
      pan.current
    )
      return
    const { graph: currentGraph, origin: currentOrigin, onChange: change } = callbacks.current
    const node =
      selection.kind === 'node'
        ? currentGraph.nodes.find((candidate) => candidate.id === selection.id)
        : currentGraph.boundaryPositions[selection.id]
    if (!node) return
    const margin = 34
    const currentZoom = currentGraph.viewport.zoom
    const left = (node.x + currentOrigin.x) * currentZoom
    const top = (node.y + currentOrigin.y) * currentZoom
    let x = currentGraph.viewport.x,
      y = currentGraph.viewport.y
    if (left < x + margin) x = left - margin
    else if (left + NODE_WIDTH * currentZoom > x + size.width - margin)
      x = left + NODE_WIDTH * currentZoom - size.width + margin
    if (top < y + margin) y = top - margin
    else if (top + NODE_HEIGHT * currentZoom > y + size.height - margin)
      y = top + NODE_HEIGHT * currentZoom - size.height + margin
    x = boundedPosition(x, 0)
    y = boundedPosition(y, 0)
    if (Math.abs(x - currentGraph.viewport.x) >= 1 || Math.abs(y - currentGraph.viewport.y) >= 1)
      change((current) => ({ ...current, viewport: { ...current.viewport, x, y } }), {
        transient: true
      })
    // A selection or canvas resize reveals the node; ordinary pan/zoom keeps the user's view.
  }, [selection, size.width, size.height])

  useEffect(() => {
    const element = scrollRef.current
    if (!element) return
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return
      event.preventDefault()
      const rect = element.getBoundingClientRect()
      const anchor = { x: event.clientX - rect.left, y: event.clientY - rect.top }
      const factor = Math.exp(-Math.max(-100, Math.min(100, event.deltaY)) * 0.008)
      callbacks.current.onChange(
        (current) => ({
          ...current,
          viewport: zoomViewport(current.viewport, current.viewport.zoom * factor, anchor)
        }),
        { transient: true }
      )
    }
    element.addEventListener('wheel', wheel, { passive: false })
    return () => element.removeEventListener('wheel', wheel)
  }, [])

  useEffect(() => {
    const release = () => {
      space.current = false
      setSpaceHeld(false)
      pan.current = null
      setPanning(false)
    }
    const keydown = (event: KeyboardEvent) => {
      if (event.code !== 'Space' || event.metaKey || event.ctrlKey || isEditingText(event.target))
        return
      if (!hovered.current && !scrollRef.current?.contains(document.activeElement)) return
      event.preventDefault()
      space.current = true
      setSpaceHeld(true)
    }
    const keyup = (event: KeyboardEvent) => {
      if (event.code === 'Space') release()
    }
    window.addEventListener('keydown', keydown)
    window.addEventListener('keyup', keyup)
    window.addEventListener('blur', release)
    return () => {
      window.removeEventListener('keydown', keydown)
      window.removeEventListener('keyup', keyup)
      window.removeEventListener('blur', release)
    }
  }, [])

  const point = (clientX: number, clientY: number) => {
    const rect = stageRef.current?.getBoundingClientRect()
    return {
      x: (clientX - (rect?.left ?? 0)) / zoom - origin.x,
      y: (clientY - (rect?.top ?? 0)) / zoom - origin.y
    }
  }
  const move = (event: PointerEvent<HTMLDivElement>) => {
    const scroller = scrollRef.current
    const currentPan = pan.current
    if (currentPan && currentPan.pointerId === event.pointerId && scroller) {
      scroller.scrollLeft = currentPan.left - (event.clientX - currentPan.x)
      scroller.scrollTop = currentPan.top - (event.clientY - currentPan.y)
      return
    }
    if (pending || connectionDrag.current) setCursor(point(event.clientX, event.clientY))
    const active = drag.current
    if (!active || active.pointerId !== event.pointerId) return
    const x = boundedPosition(
      active.x + (event.clientX - active.startX) / active.zoom,
      active.target.kind === 'boundary' ? -99000 : 24
    )
    const y = boundedPosition(
      active.y + (event.clientY - active.startY) / active.zoom,
      active.target.kind === 'boundary' ? -99000 : 40
    )
    onChange((current) => positionCard(current, active.target, { x, y }), { group: active.group })
  }
  const finishPointer = (event: PointerEvent<HTMLDivElement>) => {
    if (connectionDrag.current?.pointerId === event.pointerId) {
      const active = connectionDrag.current
      connectionDrag.current = null
      const target = document
        .elementFromPoint(event.clientX, event.clientY)
        ?.closest<HTMLElement>('[data-workflow-input]')
      if (Math.hypot(event.clientX - active.x, event.clientY - active.y) > 3) {
        if (target?.dataset.workflowInput === 'boundary') onFinish({ kind: 'boundary' })
        else if (target?.dataset.workflowInput === 'node' && target.dataset.workflowNodeId)
          onFinish({ kind: 'node', nodeId: target.dataset.workflowNodeId })
      }
      setCursor(null)
    }
    drag.current = null
    pan.current = null
    setPanning(false)
  }
  const pendingNode =
    pending?.kind === 'node'
      ? graph.nodes.find((node) => node.id === pending.nodeId)
      : pending?.kind === 'boundary'
        ? graph.boundaryPositions.input
        : undefined
  const beginCardDrag = (
    event: PointerEvent<HTMLDivElement>,
    target: CardSelection,
    position: CanvasPoint
  ) => {
    if (event.button !== 0 || (event.target instanceof Element && event.target.closest('button')))
      return
    event.preventDefault()
    event.currentTarget.focus({ preventScroll: true })
    onSelect(target)
    drag.current = {
      target,
      pointerId: event.pointerId,
      ...position,
      startX: event.clientX,
      startY: event.clientY,
      zoom,
      group: `${target.kind}-drag-${crypto.randomUUID()}`
    }
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  const moveCardKey = (event: ReactKeyboardEvent<HTMLDivElement>, target: CardSelection) => {
    if (event.target !== event.currentTarget) return
    const delta = event.shiftKey ? 40 : 10
    const offsets: Record<string, [number, number]> = {
      ArrowLeft: [-delta, 0],
      ArrowRight: [delta, 0],
      ArrowUp: [0, -delta],
      ArrowDown: [0, delta]
    }
    const offset = offsets[event.key]
    if (!offset) return
    event.preventDefault()
    onChange((current) => {
      const position =
        target.kind === 'node'
          ? current.nodes.find((node) => node.id === target.id)
          : current.boundaryPositions[target.id]
      return position
        ? positionCard(current, target, {
            x: boundedPosition(position.x + offset[0], target.kind === 'boundary' ? -99000 : 24),
            y: boundedPosition(position.y + offset[1], target.kind === 'boundary' ? -99000 : 40)
          })
        : current
    })
  }
  const beginConnectionPointer = (
    event: PointerEvent<HTMLButtonElement>,
    source: WorkflowEndpoint
  ) => {
    if (event.button !== 0) return
    event.preventDefault()
    event.stopPropagation()
    connectionDrag.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY }
    setCursor(point(event.clientX, event.clientY))
    onBegin(source)
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  const changeZoom = (next: number) =>
    onChange(
      (current) => ({
        ...current,
        viewport: zoomViewport(current.viewport, next, { x: size.width / 2, y: size.height / 2 })
      }),
      { transient: true }
    )
  const fit = () =>
    onChange(
      (current) => ({
        ...current,
        viewport: fitViewport(graphBounds(current), size.width, size.height, origin)
      }),
      { transient: true }
    )

  return (
    <div className="workflow-canvas-shell">
      <div
        ref={scrollRef}
        className={`workflow-canvas${spaceHeld ? ' is-space-held' : ''}${panning ? ' is-panning' : ''}`}
        aria-label={text('graphTitle')}
        tabIndex={0}
        onPointerEnter={() => {
          hovered.current = true
        }}
        onPointerLeave={() => {
          hovered.current = false
        }}
        onPointerMove={move}
        onPointerUp={finishPointer}
        onPointerCancel={() => {
          drag.current = null
          connectionDrag.current = null
          pan.current = null
          setPanning(false)
          setCursor(null)
        }}
        onPointerDownCapture={(event) => {
          if (!(space.current && event.button === 0) && event.button !== 1) return
          event.preventDefault()
          event.stopPropagation()
          pan.current = {
            pointerId: event.pointerId,
            x: event.clientX,
            y: event.clientY,
            left: event.currentTarget.scrollLeft,
            top: event.currentTarget.scrollTop
          }
          setPanning(true)
          event.currentTarget.setPointerCapture(event.pointerId)
        }}
        onScroll={(event) => {
          const x = Math.min(100000, event.currentTarget.scrollLeft),
            y = Math.min(100000, event.currentTarget.scrollTop)
          if (event.currentTarget.scrollLeft !== x) event.currentTarget.scrollLeft = x
          if (event.currentTarget.scrollTop !== y) event.currentTarget.scrollTop = y
          onChange(
            (current) =>
              Math.abs(current.viewport.x - x) < 1 && Math.abs(current.viewport.y - y) < 1
                ? current
                : { ...current, viewport: { ...current.viewport, x, y } },
            { transient: true }
          )
        }}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes(WORKFLOW_DRAG_TYPE)) {
            event.preventDefault()
            event.dataTransfer.dropEffect = 'copy'
          }
        }}
        onDrop={(event) => {
          const value = event.dataTransfer.getData(WORKFLOW_DRAG_TYPE)
          if (!value) return
          event.preventDefault()
          const position = point(event.clientX, event.clientY)
          onAddNode(
            boundedPosition(position.x - NODE_WIDTH / 2, 24),
            boundedPosition(position.y - NODE_HEIGHT / 2, 40),
            value === 'blank' ? undefined : value
          )
        }}
      >
        <div
          className="workflow-canvas__extent"
          style={{ width: width * zoom, height: height * zoom }}
        >
          <div
            ref={stageRef}
            className="workflow-canvas__stage"
            style={{ width, height, transform: `scale(${zoom})` }}
            onClick={(event) => {
              if (event.target === event.currentTarget) onSelect(null)
            }}
          >
            <svg
              className="workflow-canvas__edges"
              width={width}
              height={height}
              aria-label={text('connection')}
            >
              <defs>
                <marker
                  id={markerId}
                  markerWidth="7"
                  markerHeight="7"
                  refX="6.2"
                  refY="3.5"
                  orient="auto"
                  markerUnits="strokeWidth"
                >
                  <path d="M 0 0 L 7 3.5 L 0 7 z" fill="currentColor" />
                </marker>
              </defs>
              <g transform={`translate(${origin.x} ${origin.y})`}>
                {graph.flows.map((flow, index) => {
                  const geometry = geometries.get(flow.id)
                  if (!geometry) return null
                  const selected = selection?.kind === 'flow' && selection.id === flow.id
                  const label = flow.name.length > 18 ? `${flow.name.slice(0, 17)}…` : flow.name
                  const labelWidth = Math.min(
                    208,
                    [...label].reduce((sum, char) => sum + (char.charCodeAt(0) > 127 ? 11 : 6), 14)
                  )
                  return (
                    <g
                      key={flow.id}
                      className={`workflow-edge${selected ? ' is-selected' : ''}`}
                      role="button"
                      tabIndex={0}
                      aria-label={`${text('connection')} ${flow.name || String(index + 1)}`}
                      onClick={() => onSelect({ kind: 'flow', id: flow.id })}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' || event.key === ' ') {
                          event.preventDefault()
                          onSelect({ kind: 'flow', id: flow.id })
                        }
                      }}
                    >
                      <path className="workflow-edge__hit" d={geometry.path} />
                      <path
                        className="workflow-edge__line"
                        d={geometry.path}
                        markerEnd={`url(#${markerId})`}
                      />
                      {label ? (
                        <g className="workflow-edge__caption">
                          <rect
                            x={geometry.label.x - labelWidth / 2}
                            y={geometry.label.y - 12}
                            width={labelWidth}
                            height={20}
                            rx={5}
                          />
                          <text x={geometry.label.x} y={geometry.label.y + 2} textAnchor="middle">
                            {label}
                          </text>
                        </g>
                      ) : null}
                    </g>
                  )
                })}
                {graph.flows.flatMap((flow) =>
                  (geometries.get(flow.id)?.bridges ?? []).map((bridge, index) => (
                    <g
                      key={`${flow.id}-bridge-${index}`}
                      className={`workflow-crossing${selection?.kind === 'flow' && selection.id === flow.id ? ' is-selected' : ''}`}
                      data-flow-id={flow.id}
                      aria-hidden="true"
                      onClick={() => onSelect({ kind: 'flow', id: flow.id })}
                    >
                      <path className="workflow-crossing__halo" d={bridge.path} />
                      <path className="workflow-crossing__line" d={bridge.path} />
                    </g>
                  ))
                )}
                {pendingNode && cursor ? (
                  <path
                    className="workflow-edge__preview"
                    d={`M ${pendingNode.x + NODE_WIDTH} ${pendingNode.y + NODE_HEIGHT / 2} C ${pendingNode.x + NODE_WIDTH + 60} ${pendingNode.y + NODE_HEIGHT / 2}, ${cursor.x - 60} ${cursor.y}, ${cursor.x} ${cursor.y}`}
                  />
                ) : null}
              </g>
            </svg>
            {(['input', 'output'] as const).map((side) => {
              const position = graph.boundaryPositions[side]
              const label = text(side === 'input' ? 'rootInput' : 'rootOutput')
              return (
                <div
                  key={`boundary-${side}`}
                  className={`workflow-node workflow-node--root${selection?.kind === 'boundary' && selection.id === side ? ' is-selected' : ''}`}
                  style={{
                    left: position.x + origin.x,
                    top: position.y + origin.y,
                    width: NODE_WIDTH,
                    height: NODE_HEIGHT
                  }}
                  tabIndex={0}
                  role="group"
                  aria-label={label}
                  title={text('moveHint')}
                  onFocus={(event) => {
                    if (event.target === event.currentTarget)
                      onSelect({ kind: 'boundary', id: side })
                  }}
                  onClick={() => onSelect({ kind: 'boundary', id: side })}
                  onPointerDown={(event) =>
                    beginCardDrag(event, { kind: 'boundary', id: side }, position)
                  }
                  onKeyDown={(event) => moveCardKey(event, { kind: 'boundary', id: side })}
                >
                  {side === 'output' ? (
                    <button
                      type="button"
                      className="workflow-node__port workflow-node__port--input"
                      data-workflow-input="boundary"
                      aria-label={`${label} ${text('inputPort')}`}
                      title={text('inputPort')}
                      onClick={(event) => {
                        event.stopPropagation()
                        onFinish({ kind: 'boundary' })
                      }}
                    />
                  ) : null}
                  <div className="workflow-node__copy">
                    <strong>{text('rootAgent')}</strong>
                    <span>{text(side === 'input' ? 'inputLabel' : 'outputLabel')}</span>
                  </div>
                  {side === 'input' ? (
                    <button
                      type="button"
                      className="workflow-node__port workflow-node__port--output"
                      aria-label={`${label} ${text('outputPort')}`}
                      title={text('outputPort')}
                      onPointerDown={(event) => beginConnectionPointer(event, { kind: 'boundary' })}
                      onClick={(event) => {
                        event.stopPropagation()
                        if (event.detail === 0) onBegin({ kind: 'boundary' })
                      }}
                    />
                  ) : null}
                </div>
              )
            })}
            {graph.nodes.map((node) => (
              <div
                key={node.id}
                className={`workflow-node${selection?.kind === 'node' && selection.id === node.id ? ' is-selected' : ''}`}
                style={{
                  left: node.x + origin.x,
                  top: node.y + origin.y,
                  width: NODE_WIDTH,
                  height: NODE_HEIGHT
                }}
                tabIndex={0}
                role="group"
                aria-label={`${text('node')} ${node.name || text('newNode')}`}
                title={text('moveHint')}
                onFocus={(event) => {
                  if (event.target === event.currentTarget) onSelect({ kind: 'node', id: node.id })
                }}
                onClick={() => onSelect({ kind: 'node', id: node.id })}
                onPointerDown={(event) => beginCardDrag(event, { kind: 'node', id: node.id }, node)}
                onKeyDown={(event) => moveCardKey(event, { kind: 'node', id: node.id })}
              >
                <button
                  type="button"
                  className="workflow-node__port workflow-node__port--input"
                  data-workflow-input="node"
                  data-workflow-node-id={node.id}
                  aria-label={`${node.name} ${text('inputPort')}`}
                  title={text('inputPort')}
                  onClick={(event) => {
                    event.stopPropagation()
                    onFinish({ kind: 'node', nodeId: node.id })
                  }}
                />
                <div className="workflow-node__copy workflow-node__copy--configurable">
                  <strong>{node.name || text('newNode')}</strong>
                  <span title={workflowNodeModelLabel(node, templates, models, text)}>
                    {workflowNodeModelLabel(node, templates, models, text)}
                  </span>
                </div>
                <button
                  type="button"
                  className="workflow-node__configure"
                  data-configure-node={node.id}
                  aria-label={`${text('configureNode')} ${node.name || text('newNode')}`}
                  title={text('configureNode')}
                  onPointerDown={(event) => event.stopPropagation()}
                  onClick={(event) => {
                    event.stopPropagation()
                    onConfigureNode(node.id)
                  }}
                >
                  <Settings size={14} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  className="workflow-node__port workflow-node__port--output"
                  aria-label={`${node.name} ${text('outputPort')}`}
                  title={text('outputPort')}
                  onPointerDown={(event) =>
                    beginConnectionPointer(event, { kind: 'node', nodeId: node.id })
                  }
                  onClick={(event) => {
                    event.stopPropagation()
                    if (event.detail === 0) onBegin({ kind: 'node', nodeId: node.id })
                  }}
                />
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="workflow-canvas-controls" role="group" aria-label={text('resetView')}>
        <button
          type="button"
          title={text('zoomOut')}
          aria-label={text('zoomOut')}
          disabled={zoom <= 0.25}
          onClick={() => changeZoom(zoom - 0.1)}
        >
          <Minus size={14} />
        </button>
        <button
          type="button"
          className="workflow-canvas-controls__percentage"
          title="100%"
          aria-label="100%"
          onClick={() => changeZoom(1)}
        >
          {Math.round(zoom * 100)}%
        </button>
        <button
          type="button"
          title={text('zoomIn')}
          aria-label={text('zoomIn')}
          disabled={zoom >= 2}
          onClick={() => changeZoom(zoom + 0.1)}
        >
          <Plus size={14} />
        </button>
        <span className="workflow-canvas-controls__separator" />
        <button
          type="button"
          title={text('resetView')}
          aria-label={text('resetView')}
          onClick={fit}
        >
          <Maximize2 size={14} />
        </button>
      </div>
    </div>
  )
}
