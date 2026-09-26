import { AccountAvatar } from '../auth/AccountAvatar'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { optimizeWorkflowLayout } from './workflowAnchorLayout'
import { anchorAtPoint, anchorPoint, type FlowEnd } from './workflowAnchors'
import { AgentAvatar } from '../agentCollaboration/AgentAvatar'
import type {
  WorkflowDefinition,
  WorkflowEndpoint,
  WorkflowAnchor,
  WorkflowFlow
} from '@mycopilot/protocol'
import { Maximize2, Minus, Plus, WandSparkles } from 'lucide-react'
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
import { NODE_HEIGHT, NODE_WIDTH, WORKFLOW_DRAG_TYPE, workflowNodeSize } from './workflowAuthoring'
import {
  fitViewport,
  canvasOrigin,
  graphFlowLayout,
  graphBounds,
  zoomViewport,
  type CanvasPoint
} from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import {
  workflowNodeModelLabel,
  workflowNodeLabel,
  type WorkflowModelDisplay
} from './workflowModelPresentation'
import './workflowCanvas.css'

type CardSelection = { kind: 'node'; id: string } | { kind: 'boundary'; id: 'input' }
export type WorkflowSelection = CardSelection | { kind: 'flow'; id: string } | null
export interface WorkflowCanvasChangeOptions {
  group?: string
  transient?: boolean
}
interface Props {
  graph: WorkflowDefinition
  models: readonly WorkflowModelDisplay[]
  selection: WorkflowSelection
  pending: WorkflowEndpoint | null
  pendingAnchor?: WorkflowAnchor
  connectionMode: boolean
  text: WorkflowText
  onChange: (
    update: (graph: WorkflowDefinition) => WorkflowDefinition,
    options?: WorkflowCanvasChangeOptions
  ) => void
  onSelect: (selection: WorkflowSelection) => void
  onConfigureNode: (id: string) => void
  onAddNode: (x: number, y: number, nodeType?: string) => void
  onBegin: (endpoint: WorkflowEndpoint, anchor?: WorkflowAnchor) => void
  onFinish: (endpoint: WorkflowEndpoint, anchor?: WorkflowAnchor) => void
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
  models,
  selection,
  pending,
  pendingAnchor,
  connectionMode,
  text,
  onChange,
  onSelect,
  onConfigureNode,
  onAddNode,
  onBegin,
  onFinish
}: Props) {
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || text('currentUser')
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
  const anchorDrag = useRef<{
    pointerId: number
    flowId: string
    endpoint: WorkflowEndpoint
    end: FlowEnd
    group: string
  } | null>(null)
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
  const { graph: layoutGraph, geometries } = useMemo(
    () =>
      graphFlowLayout({
        nodes: graph.nodes,
        flows: graph.flows,
        boundaryPositions: graph.boundaryPositions
      }),
    [graph.nodes, graph.flows, graph.boundaryPositions]
  )
  const flowAnchors = layoutGraph.flows.flatMap((flow, index) =>
    (['source', 'target'] as const).map((end) => ({
      flow,
      index,
      end,
      position: anchorPoint(
        layoutGraph,
        flow[end],
        end,
        flow[end === 'source' ? 'sourceAnchor' : 'targetAnchor']
      )
    }))
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
  const fitAfterLayout = useRef(false)
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
    if (fitAfterLayout.current) {
      fitAfterLayout.current = false
      onChange(
        (current) => ({
          ...current,
          viewport: fitViewport(bounds, size.width, size.height, origin)
        }),
        { transient: true }
      )
      return
    }
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
  }, [origin, onChange, bounds, size.width, size.height])

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
    const dimensions = workflowNodeSize('kind' in node ? node : undefined)
    const margin = 34
    const currentZoom = currentGraph.viewport.zoom
    const left = (node.x + currentOrigin.x) * currentZoom
    const top = (node.y + currentOrigin.y) * currentZoom
    let x = currentGraph.viewport.x,
      y = currentGraph.viewport.y
    if (left < x + margin) x = left - margin
    else if (left + dimensions.width * currentZoom > x + size.width - margin)
      x = left + dimensions.width * currentZoom - size.width + margin
    if (top < y + margin) y = top - margin
    else if (top + dimensions.height * currentZoom > y + size.height - margin)
      y = top + dimensions.height * currentZoom - size.height + margin
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
    const position = point(event.clientX, event.clientY)
    if (pending) setCursor(position)
    const handle = anchorDrag.current
    if (handle && handle.pointerId === event.pointerId) {
      onChange(
        (current) => ({
          ...current,
          flows: current.flows.map((flow) =>
            flow.id === handle.flowId
              ? {
                  ...flow,
                  [handle.end === 'source' ? 'sourceAnchor' : 'targetAnchor']: anchorAtPoint(
                    current,
                    handle.endpoint,
                    handle.end,
                    position
                  )
                }
              : flow
          )
        }),
        { group: handle.group }
      )
      return
    }
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
    if (anchorDrag.current?.pointerId === event.pointerId) anchorDrag.current = null
    drag.current = null
    pan.current = null
    setPanning(false)
  }
  const pendingPoint = pending ? anchorPoint(graph, pending, 'source', pendingAnchor) : null
  const clickCard = (target: CardSelection, client?: CanvasPoint) => {
    if (!connectionMode) {
      onSelect(target)
      return
    }
    const endpoint: WorkflowEndpoint =
      target.kind === 'boundary' ? { kind: 'boundary' } : { kind: 'node', nodeId: target.id }
    if (target.kind === 'boundary' && pending !== null) return
    if (pending) onFinish(endpoint)
    else {
      onBegin(endpoint)
      if (client) setCursor(point(client.x, client.y))
    }
  }
  const beginCardDrag = (
    event: PointerEvent<HTMLDivElement>,
    target: CardSelection,
    position: CanvasPoint
  ) => {
    if (
      connectionMode ||
      event.button !== 0 ||
      (event.target instanceof Element && event.target.closest('button'))
    )
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
    if (connectionMode) {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault()
        clickCard(target)
      }
      return
    }
    if (event.key === 'Enter' && target.kind === 'node') {
      event.preventDefault()
      onConfigureNode(target.id)
      return
    }
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
  const beginAnchorDrag = (
    event: PointerEvent<HTMLButtonElement>,
    flow: WorkflowFlow,
    end: FlowEnd
  ) => {
    if (event.button !== 0) return
    event.stopPropagation()
    event.preventDefault()
    if (connectionMode) {
      clickCard(
        flow[end].kind === 'boundary'
          ? { kind: 'boundary', id: 'input' }
          : { kind: 'node', id: (flow[end] as { nodeId: string }).nodeId },
        { x: event.clientX, y: event.clientY }
      )
      return
    }
    onSelect({ kind: 'flow', id: flow.id })
    anchorDrag.current = {
      pointerId: event.pointerId,
      flowId: flow.id,
      endpoint: flow[end],
      end,
      group: `anchor-${crypto.randomUUID()}`
    }
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
        className={`workflow-canvas${spaceHeld ? ' is-space-held' : ''}${panning ? ' is-panning' : ''}${connectionMode ? ' is-connecting' : ''}`}
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
          anchorDrag.current = null
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
          const dimensions = workflowNodeSize(
            value.startsWith('gate:') ? { kind: 'inputGate' } : undefined
          )
          onAddNode(
            boundedPosition(position.x - dimensions.width / 2, 24),
            boundedPosition(position.y - dimensions.height / 2, 40),
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
                  return (
                    <g
                      key={flow.id}
                      className={`workflow-edge${selected ? ' is-selected' : ''}`}
                      role="button"
                      tabIndex={0}
                      aria-label={`${text('connection')} ${index + 1}`}
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
                      {flow.name ? (
                        <text
                          className="workflow-edge__label"
                          x={geometry.label.x}
                          y={geometry.label.y - 7}
                          textAnchor="middle"
                        >
                          <title>{flow.name}</title>
                          {flow.name.length > 24 ? `${flow.name.slice(0, 24)}…` : flow.name}
                        </text>
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
                {pendingPoint && cursor ? (
                  <path
                    className="workflow-edge__preview"
                    d={`M ${pendingPoint.x} ${pendingPoint.y} L ${cursor.x} ${cursor.y}`}
                  />
                ) : null}
              </g>
            </svg>
            {(['input'] as const).map((side) => {
              const position = graph.boundaryPositions[side]
              const label = text('rootInput')
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
                  title={text(
                    connectionMode ? (pending ? 'connecting' : 'chooseSource') : 'moveHint'
                  )}
                  onFocus={(event) => {
                    if (event.target === event.currentTarget)
                      onSelect({ kind: 'boundary', id: side })
                  }}
                  onClick={(event) =>
                    clickCard(
                      { kind: 'boundary', id: side },
                      { x: event.clientX, y: event.clientY }
                    )
                  }
                  onPointerDown={(event) =>
                    beginCardDrag(event, { kind: 'boundary', id: side }, position)
                  }
                  onKeyDown={(event) => moveCardKey(event, { kind: 'boundary', id: side })}
                >
                  <span className="workflow-node__avatar workflow-user-avatar">
                    <AccountAvatar src={profile?.avatarDataUrl} />
                  </span>
                  <div className="workflow-node__copy">
                    <strong>{userName}</strong>
                    <span>{text('rootInput')}</span>
                  </div>
                </div>
              )
            })}
            {graph.nodes.map((node) => (
              <div
                key={node.id}
                data-workflow-node-id={node.id}
                className={`workflow-node${node.kind === 'inputGate' || node.kind === 'outputGate' ? ` workflow-node--gate workflow-node--${node.kind}` : ''}${selection?.kind === 'node' && selection.id === node.id ? ' is-selected' : ''}`}
                style={{
                  left: node.x + origin.x,
                  top: node.y + origin.y,
                  width: workflowNodeSize(node).width,
                  height: workflowNodeSize(node).height
                }}
                tabIndex={0}
                role="group"
                aria-label={`${text('node')} ${workflowNodeLabel(node, text, userName)}`}
                title={text(
                  connectionMode ? (pending ? 'connecting' : 'chooseSource') : 'configureHint'
                )}
                onFocus={(event) => {
                  if (event.target === event.currentTarget) onSelect({ kind: 'node', id: node.id })
                }}
                onClick={(event) => {
                  if (event.detail < 2)
                    clickCard({ kind: 'node', id: node.id }, { x: event.clientX, y: event.clientY })
                }}
                onDoubleClick={(event) => {
                  event.stopPropagation()
                  if (!connectionMode) onConfigureNode(node.id)
                }}
                onPointerDown={(event) => beginCardDrag(event, { kind: 'node', id: node.id }, node)}
                onKeyDown={(event) => moveCardKey(event, { kind: 'node', id: node.id })}
              >
                {node.kind === 'inputGate' || node.kind === 'outputGate' ? (
                  <svg className="workflow-gate-shape" viewBox="0 0 64 56" aria-hidden="true">
                    <path
                      d={
                        node.kind === 'inputGate'
                          ? 'M 2 2 L 62 28 L 2 54 Z'
                          : 'M 62 2 L 2 28 L 62 54 Z'
                      }
                    />
                  </svg>
                ) : null}
                {node.kind === 'agent' || node.kind === 'user' ? (
                  <>
                    {node.kind === 'user' ? (
                      <span className="workflow-node__avatar workflow-user-avatar">
                        <AccountAvatar src={profile?.avatarDataUrl} />
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
                  </>
                ) : null}
              </div>
            ))}
            {flowAnchors.map(({ flow, index, end, position }) => {
              if (!position) return null
              const label = `${text('connection')} ${index + 1} ${text(end === 'source' ? 'source' : 'target')}`
              return (
                <button
                  key={`${flow.id}:${end}`}
                  type="button"
                  className={`workflow-anchor-handle${selection?.kind === 'flow' && selection.id === flow.id ? ' is-selected' : ''}`}
                  style={{ left: position.x + origin.x, top: position.y + origin.y }}
                  aria-label={label}
                  title={label}
                  onClick={(event) => {
                    event.stopPropagation()
                    if (!connectionMode) onSelect({ kind: 'flow', id: flow.id })
                  }}
                  onPointerDown={(event) => beginAnchorDrag(event, flow, end)}
                  onKeyDown={(event) => {
                    const delta = (
                      {
                        ArrowLeft: [-10, 0],
                        ArrowRight: [10, 0],
                        ArrowUp: [0, -10],
                        ArrowDown: [0, 10]
                      } as Record<string, number[]>
                    )[event.key]
                    if (!delta || connectionMode) return
                    event.preventDefault()
                    event.stopPropagation()
                    onChange((current) => ({
                      ...current,
                      flows: current.flows.map((f) =>
                        f.id === flow.id
                          ? {
                              ...f,
                              [end === 'source' ? 'sourceAnchor' : 'targetAnchor']: anchorAtPoint(
                                current,
                                f[end],
                                end,
                                { x: position.x + delta[0], y: position.y + delta[1] }
                              )
                            }
                          : f
                      )
                    }))
                  }}
                />
              )
            })}
          </div>
        </div>
      </div>
      <div className="workflow-canvas-actions">
        <button
          type="button"
          className="workflow-canvas-optimize"
          title={text('optimizeLayout')}
          aria-label={text('optimizeLayout')}
          disabled={!graph.nodes.length && !graph.flows.length}
          onClick={() => {
            fitAfterLayout.current = true
            onChange(optimizeWorkflowLayout)
          }}
        >
          <WandSparkles size={15} />
        </button>
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
    </div>
  )
}
