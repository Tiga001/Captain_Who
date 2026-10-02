import { AgentAvatar } from '../agentCollaboration/AgentAvatar'
import type { WorkflowDefinition } from '@mycopilot/protocol'
import { Maximize2, Minus, Plus, WandSparkles } from 'lucide-react'
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { NODE_WIDTH, NODE_HEIGHT, WORKFLOW_DRAG_TYPE } from './workflowAuthoring'
import { fitViewport, canvasOrigin, graphBounds, zoomViewport } from './workflowCanvasGeometry'
import { optimizeWorkflowLayout } from './workflowAutoLayout'
import { WorkflowNodeRankBadge } from './WorkflowNodeRankBadge'
import type { WorkflowText } from './workflowText'
import {
  workflowNodeModelLabel,
  workflowNodeLabel,
  type WorkflowModelDisplay
} from './workflowModelPresentation'
import {
  addWorkflowDepartment,
  canPlaceWorkflowDepartments,
  MAX_WORKFLOW_DEPARTMENTS,
  MIN_DEPARTMENT_HEIGHT,
  MIN_DEPARTMENT_WIDTH,
  moveWorkflowDepartment,
  normalizeWorkflowDepartments,
  workflowDepartmentLevel
} from './workflowDepartments'
import { WORKFLOW_CONVERSATION_DRAG_TYPE } from './project/projectWorkflowText'
import './workflowCanvas.css'

export type WorkflowSelection = { kind: 'node' | 'department'; id: string } | null
export interface WorkflowCanvasChangeOptions {
  group?: string
  transient?: boolean
}
export interface WorkflowConversationBindings {
  values: Readonly<Record<string, string | null>>
  options: readonly { id: string; title: string }[]
  label: string
  emptyLabel: string
  onChange: (nodeId: string, conversationId: string | null) => void
}
interface Props {
  canvasActions?: ReactNode
  conversationBindings?: WorkflowConversationBindings
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
  onAddNode: (x: number, y: number) => void
  departmentDrawing?: boolean
  onDepartmentDrawingChange?: (drawing: boolean) => void
}
export function WorkflowCanvas({
  canvasActions,
  graph,
  models,
  selection,
  text,
  onChange,
  onSelect,
  onConfigureNode,
  onAddNode,
  departmentDrawing = false,
  onDepartmentDrawingChange,
  conversationBindings
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ width: 900, height: 550 })
  const [panning, setPanning] = useState(false)
  const [bindingTarget, setBindingTarget] = useState<string | null>(null)
  const initialBindingView = useRef<string | null>(null)
  const [departmentDraft, setDepartmentDraft] = useState<{
    x: number
    y: number
    width: number
    height: number
  } | null>(null)
  const [invalidDepartment, setInvalidDepartment] = useState(false)
  const drawing = useRef<{
    pointer: number
    x: number
    y: number
    current: { x: number; y: number; width: number; height: number }
  } | null>(null)
  const drag = useRef<{
    kind: 'node' | 'department' | 'resize'
    id: string
    pointer: number
    x: number
    y: number
    left: number
    top: number
    group: string
    initial: WorkflowDefinition
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
    if (
      !conversationBindings ||
      initialBindingView.current === graph.id ||
      !element?.clientWidth ||
      !element.clientHeight
    )
      return
    initialBindingView.current = graph.id
    onChange(
      (current) => {
        const bounds = graphBounds(current)
        return {
          ...current,
          viewport: fitViewport(
            bounds,
            element.clientWidth,
            element.clientHeight,
            canvasOrigin(bounds)
          )
        }
      },
      { transient: true }
    )
  }, [graph.id, conversationBindings, onChange])
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
  useEffect(() => {
    if (!departmentDrawing) return
    setInvalidDepartment(false)
    scrollRef.current?.focus()
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      drawing.current = null
      setDepartmentDraft(null)
      setInvalidDepartment(false)
      onDepartmentDrawingChange?.(false)
    }
    window.addEventListener('keydown', cancel)
    return () => window.removeEventListener('keydown', cancel)
  }, [departmentDrawing, onDepartmentDrawingChange])
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
        className={`workflow-canvas${panning ? ' is-panning' : ''}${departmentDrawing ? ' is-drawing-department' : ''}`}
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
        onPointerDownCapture={(event) => {
          if (!departmentDrawing || event.button !== 0) return
          if ((graph.departments?.length ?? 0) >= MAX_WORKFLOW_DEPARTMENTS) return
          event.preventDefault()
          event.stopPropagation()
          const rect = event.currentTarget.getBoundingClientRect()
          const x = (event.clientX - rect.left + graph.viewport.x) / zoom - origin.x
          const y = (event.clientY - rect.top + graph.viewport.y) / zoom - origin.y
          const current = { x, y, width: 0, height: 0 }
          drawing.current = { pointer: event.pointerId, x, y, current }
          setDepartmentDraft(current)
          setInvalidDepartment(false)
          event.currentTarget.setPointerCapture(event.pointerId)
        }}
        onPointerDown={(event) => {
          if (event.button !== 0 && event.button !== 1) return
          if (
            (event.target as Element).closest(
              '[data-workflow-node-id], [data-workflow-department-id]'
            )
          )
            return
          event.preventDefault()
          setInvalidDepartment(false)
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
          const drawingItem = drawing.current
          if (drawingItem && drawingItem.pointer === event.pointerId) {
            const rect = event.currentTarget.getBoundingClientRect()
            const x = (event.clientX - rect.left + graph.viewport.x) / zoom - origin.x
            const y = (event.clientY - rect.top + graph.viewport.y) / zoom - origin.y
            const current = {
              x: Math.min(drawingItem.x, x),
              y: Math.min(drawingItem.y, y),
              width: Math.abs(x - drawingItem.x),
              height: Math.abs(y - drawingItem.y)
            }
            drawingItem.current = current
            setDepartmentDraft(current)
            setInvalidDepartment(
              !canPlaceWorkflowDepartments([
                ...(graph.departments ?? []),
                { ...current, id: '__draft__', name: '', parentId: null }
              ])
            )
            return
          }
          const item = drag.current
          if (item && item.pointer === event.pointerId) {
            if (item.kind !== 'node') {
              const dx = (event.clientX - item.x) / zoom
              const dy = (event.clientY - item.y) / zoom
              const base = { ...item.initial, viewport: graph.viewport }
              const next =
                item.kind === 'department'
                  ? moveWorkflowDepartment(base, item.id, dx, dy)
                  : {
                      ...base,
                      departments: base.departments?.map((department) =>
                        department.id === item.id
                          ? {
                              ...department,
                              width: Math.max(MIN_DEPARTMENT_WIDTH, department.width + dx),
                              height: Math.max(MIN_DEPARTMENT_HEIGHT, department.height + dy)
                            }
                          : department
                      )
                    }
              const valid =
                item.kind === 'department'
                  ? next !== base
                  : canPlaceWorkflowDepartments(next.departments ?? [])
              setInvalidDepartment(!valid)
              if (valid) onChange(() => normalizeWorkflowDepartments(next), { group: item.group })
              return
            }
            const x = Math.max(
                -100000,
                Math.min(100000, item.left + (event.clientX - item.x) / zoom)
              ),
              y = Math.max(-100000, Math.min(100000, item.top + (event.clientY - item.y) / zoom))
            const initialNode = item.initial.nodes.find((node) => node.id === item.id)
            onChange(
              (current) =>
                normalizeWorkflowDepartments({
                  ...current,
                  nodes: current.nodes.map((n) =>
                    n.id === item.id
                      ? { ...n, managementRole: initialNode?.managementRole, x, y }
                      : n
                  )
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
          const draft = drawing.current?.current
          if (
            draft &&
            draft.width >= MIN_DEPARTMENT_WIDTH &&
            draft.height >= MIN_DEPARTMENT_HEIGHT
          ) {
            const department = {
              ...draft,
              id: crypto.randomUUID(),
              name: text('newDepartment'),
              parentId: null
            }
            if (canPlaceWorkflowDepartments([...(graph.departments ?? []), department])) {
              onChange((current) => addWorkflowDepartment(current, department))
              onSelect({ kind: 'department', id: department.id })
              onDepartmentDrawingChange?.(false)
              setInvalidDepartment(false)
            }
          }
          drawing.current = null
          setDepartmentDraft(null)
          drag.current = null
          pan.current = null
          setPanning(false)
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            event.currentTarget.releasePointerCapture(event.pointerId)
        }}
        onPointerCancel={() => {
          drawing.current = null
          setDepartmentDraft(null)
          drag.current = null
          pan.current = null
          setPanning(false)
          setInvalidDepartment(false)
        }}
        onDragOver={(event) => {
          if (event.dataTransfer.types.includes(WORKFLOW_DRAG_TYPE)) {
            event.preventDefault()
            event.dataTransfer.dropEffect = 'copy'
          }
        }}
        onDrop={(event) => {
          const value = event.dataTransfer.getData(WORKFLOW_DRAG_TYPE)
          if (value !== 'blank') return
          event.preventDefault()
          const rect = event.currentTarget.getBoundingClientRect()
          onAddNode(
            (event.clientX - rect.left + graph.viewport.x) / zoom - origin.x - NODE_WIDTH / 2,
            (event.clientY - rect.top + graph.viewport.y) / zoom - origin.y - NODE_HEIGHT / 2
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
            {[...(graph.departments ?? [])]
              .sort((a, b) => b.width * b.height - a.width * a.height)
              .map((department) => (
                <div
                  key={department.id}
                  data-workflow-department-id={department.id}
                  className={`workflow-department${selection?.kind === 'department' && selection.id === department.id ? ' is-selected' : ''}`}
                  style={{
                    left: department.x + origin.x,
                    top: department.y + origin.y,
                    width: department.width,
                    height: department.height
                  }}
                >
                  <button
                    type="button"
                    className="workflow-department__heading"
                    aria-label={`${department.name} · ${text('departmentLevel').replace('{level}', String(workflowDepartmentLevel(graph, department.id)))}`}
                    onClick={() => onSelect({ kind: 'department', id: department.id })}
                    onPointerDown={(event) => {
                      if (event.button !== 0) return
                      event.preventDefault()
                      event.stopPropagation()
                      event.currentTarget.focus()
                      setInvalidDepartment(false)
                      onSelect({ kind: 'department', id: department.id })
                      drag.current = {
                        kind: 'department',
                        id: department.id,
                        pointer: event.pointerId,
                        x: event.clientX,
                        y: event.clientY,
                        left: department.x,
                        top: department.y,
                        group: `department-move-${crypto.randomUUID()}`,
                        initial: graph
                      }
                      scrollRef.current?.setPointerCapture(event.pointerId)
                    }}
                    onKeyDown={(event) => {
                      const delta = {
                        ArrowLeft: [-1, 0],
                        ArrowRight: [1, 0],
                        ArrowUp: [0, -1],
                        ArrowDown: [0, 1]
                      }[event.key]
                      if (!delta) return
                      event.preventDefault()
                      const step = event.shiftKey ? 20 : 4
                      onChange(
                        (current) =>
                          moveWorkflowDepartment(
                            current,
                            department.id,
                            delta[0] * step,
                            delta[1] * step
                          ),
                        { group: `department-key-${department.id}` }
                      )
                    }}
                  >
                    <strong>{department.name}</strong>
                    <span>
                      {text('departmentLevel').replace(
                        '{level}',
                        String(workflowDepartmentLevel(graph, department.id))
                      )}
                    </span>
                  </button>
                  {selection?.kind === 'department' && selection.id === department.id && (
                    <button
                      type="button"
                      className="workflow-department__resize"
                      aria-label={text('resizeDepartment')}
                      onPointerDown={(event) => {
                        if (event.button !== 0) return
                        event.preventDefault()
                        event.stopPropagation()
                        setInvalidDepartment(false)
                        drag.current = {
                          kind: 'resize',
                          id: department.id,
                          pointer: event.pointerId,
                          x: event.clientX,
                          y: event.clientY,
                          left: department.x,
                          top: department.y,
                          group: `department-resize-${crypto.randomUUID()}`,
                          initial: graph
                        }
                        scrollRef.current?.setPointerCapture(event.pointerId)
                      }}
                      onKeyDown={(event) => {
                        const delta = {
                          ArrowLeft: [-1, 0],
                          ArrowRight: [1, 0],
                          ArrowUp: [0, -1],
                          ArrowDown: [0, 1]
                        }[event.key]
                        if (!delta) return
                        event.preventDefault()
                        const step = event.shiftKey ? 20 : 4
                        onChange(
                          (current) => {
                            const departments = current.departments?.map((item) =>
                              item.id === department.id
                                ? {
                                    ...item,
                                    width: Math.max(
                                      MIN_DEPARTMENT_WIDTH,
                                      item.width + delta[0] * step
                                    ),
                                    height: Math.max(
                                      MIN_DEPARTMENT_HEIGHT,
                                      item.height + delta[1] * step
                                    )
                                  }
                                : item
                            )
                            return canPlaceWorkflowDepartments(departments ?? [])
                              ? normalizeWorkflowDepartments({ ...current, departments })
                              : current
                          },
                          { group: `department-resize-key-${department.id}` }
                        )
                      }}
                    />
                  )}
                </div>
              ))}
            {departmentDraft && (
              <div
                className={`workflow-department workflow-department--draft${invalidDepartment ? ' is-invalid' : ''}`}
                style={{
                  left: departmentDraft.x + origin.x,
                  top: departmentDraft.y + origin.y,
                  width: departmentDraft.width,
                  height: departmentDraft.height
                }}
              />
            )}
            {graph.nodes.map((node) => (
              <div
                key={node.id}
                data-workflow-node-id={node.id}
                className={`workflow-node${selection?.kind === 'node' && selection.id === node.id ? ' is-selected' : ''}${bindingTarget === node.id ? ' is-drop-target' : ''}${conversationBindings?.values[node.id] ? ' is-bound' : ''}`}
                onDragOver={(event) => {
                  if (
                    !conversationBindings ||
                    node.kind !== 'agent' ||
                    !event.dataTransfer.types.includes(WORKFLOW_CONVERSATION_DRAG_TYPE)
                  )
                    return
                  event.preventDefault()
                  event.stopPropagation()
                  event.dataTransfer.dropEffect = 'link'
                  setBindingTarget(node.id)
                }}
                onDragLeave={() => setBindingTarget(null)}
                onDrop={(event) => {
                  if (!conversationBindings || node.kind !== 'agent') return
                  const id = event.dataTransfer.getData(WORKFLOW_CONVERSATION_DRAG_TYPE)
                  if (!id) return
                  event.preventDefault()
                  event.stopPropagation()
                  setBindingTarget(null)
                  onSelect({ kind: 'node', id: node.id })
                  conversationBindings.onChange(node.id, id)
                }}
                style={{
                  left: node.x + origin.x,
                  top: node.y + origin.y,
                  width: NODE_WIDTH,
                  height: NODE_HEIGHT
                }}
                role="group"
                tabIndex={0}
                aria-label={`${text('node')} ${workflowNodeLabel(node, text)}`}
                onClick={() => onSelect({ kind: 'node', id: node.id })}
                onDoubleClick={() => onConfigureNode(node.id)}
                onPointerDown={(event) => {
                  if (event.button !== 0) return
                  event.stopPropagation()
                  event.preventDefault()
                  event.currentTarget.focus()
                  setInvalidDepartment(false)
                  onSelect({ kind: 'node', id: node.id })
                  drag.current = {
                    kind: 'node',
                    id: node.id,
                    pointer: event.pointerId,
                    x: event.clientX,
                    y: event.clientY,
                    left: node.x,
                    top: node.y,
                    group: `node-move-${crypto.randomUUID()}`,
                    initial: graph
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
                    (current) =>
                      normalizeWorkflowDepartments({
                        ...current,
                        nodes: current.nodes.map((n) =>
                          n.id === node.id
                            ? {
                                ...n,
                                x: Math.max(-100000, Math.min(100000, n.x + d[0] * step)),
                                y: Math.max(-100000, Math.min(100000, n.y + d[1] * step))
                              }
                            : n
                        )
                      }),
                    { group: `node-key-${node.id}` }
                  )
                }}
              >
                <WorkflowNodeRankBadge node={node} text={text} />
                <AgentAvatar agentId={node.id} className="workflow-node__avatar" />
                <div className="workflow-node__copy workflow-node__copy--configurable">
                  <strong>{workflowNodeLabel(node, text)}</strong>
                  <span title={workflowNodeModelLabel(node, models, text)}>
                    {conversationBindings
                      ? (conversationBindings.options.find(
                          (item) => item.id === conversationBindings.values[node.id]
                        )?.title ?? conversationBindings.emptyLabel)
                      : workflowNodeModelLabel(node, models, text)}
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
      {invalidDepartment && (
        <div className="workflow-canvas-hint" role="status">
          {text('invalidDepartmentOverlap')}
        </div>
      )}
      <div className="workflow-canvas-actions">
        <div className="workflow-canvas-actions__tools">
          {canvasActions}
          <button
            type="button"
            className="workflow-canvas-optimize"
            aria-label={text('optimizeLayout')}
            title={text('optimizeLayout')}
            disabled={!graph.nodes.length && !graph.departments?.length}
            onClick={() => {
              onDepartmentDrawingChange?.(false)
              drawing.current = null
              setDepartmentDraft(null)
              setInvalidDepartment(false)
              onChange((current) => {
                const arranged = optimizeWorkflowLayout(current)
                const bounds = graphBounds(arranged)
                return {
                  ...arranged,
                  viewport: fitViewport(bounds, size.width, size.height, canvasOrigin(bounds))
                }
              })
            }}
          >
            <WandSparkles size={18} />
          </button>
        </div>
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
