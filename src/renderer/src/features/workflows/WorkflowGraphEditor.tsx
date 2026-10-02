import { Tooltip } from '../../components/overlay/Tooltip'
import { Building2, SquareDashed, Plus, Settings, Trash2, X } from 'lucide-react'
import { useEffect, useId, useRef, useState, type ClipboardEvent, type ReactNode } from 'react'
import type { WorkflowDefinition, WorkflowNode, WorkflowAgentNode } from '@mycopilot/protocol'
import { createWorkflowNode, workflowNodeSize, WORKFLOW_DRAG_TYPE } from './workflowAuthoring'
import {
  WorkflowCanvas,
  type WorkflowSelection,
  type WorkflowConversationBindings
} from './WorkflowCanvas'
import {
  WORKFLOW_NODE_CLIPBOARD_TYPE,
  serializeWorkflowNode,
  readWorkflowNodeClipboard,
  duplicateWorkflowNode
} from './workflowNodeClipboard'
import { SettingsSelect } from '../settings/components/SettingsSelect'
import { AgentAvatar } from '../agentCollaboration/AgentAvatar'
import { CHAT_PERMISSION_PRESENTATIONS } from '../chat/chatPermissionPresentation'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { WorkflowOptionPicker } from './WorkflowOptionPicker'
import { canvasOrigin, graphBounds } from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import { useModelSettings } from '../../config/ModelSettingsProvider'
import { ModelConfigPicker } from '../modelSelection/ModelConfigPicker'
import { formatModelConfigLabel } from '../modelSelection/modelConfigPresentation'
import { workflowNodeModelLabel, workflowNodeLabel } from './workflowModelPresentation'
import {
  normalizeWorkflowDepartments,
  removeWorkflowDepartment,
  workflowDepartmentPath
} from './workflowDepartments'
import './workflowGraphEditor.css'
export interface WorkflowGraphEditorProps {
  definition: WorkflowDefinition
  canvasActions?: ReactNode
  conversationBindings?: WorkflowConversationBindings
  disabled?: boolean
  text: WorkflowText
  onChange: (
    update: (current: WorkflowDefinition) => WorkflowDefinition,
    options?: { group?: string; transient?: boolean }
  ) => void
}
export function WorkflowGraphEditor({
  definition: graph,
  text,
  onChange,
  canvasActions,
  conversationBindings,
  disabled = false
}: WorkflowGraphEditorProps) {
  const { t } = useFrontendConfig()
  const { models, enabledModels } = useModelSettings()
  const [selection, setSelection] = useState<WorkflowSelection>(null)
  const [configuredNodeId, setConfiguredNodeId] = useState<string | null>(null)
  const [inspectorTab, setInspectorTab] = useState<'task' | 'rank'>('task')
  const [departmentDrawing, setDepartmentDrawing] = useState(false)
  const inspectorId = useId()
  const [panel, setPanel] = useState(false)
  const canvasRef = useRef<HTMLDivElement>(null)
  const toolbarRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!panel) return
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !toolbarRef.current?.contains(event.target))
        setPanel(false)
    }
    document.addEventListener('pointerdown', outside)
    return () => document.removeEventListener('pointerdown', outside)
  }, [panel])
  const selectedNode = graph.nodes.find((n) => selection?.kind === 'node' && n.id === selection.id)
  const configuredNode = graph.nodes.find((n) => n.id === configuredNodeId)
  const selectedDepartment = graph.departments?.find(
    (department) => selection?.kind === 'department' && department.id === selection.id
  )
  const parentDepartmentLabel = selectedDepartment?.parentId
    ? workflowDepartmentPath(graph, selectedDepartment.parentId).join(' / ')
    : text('directOrganization')
  const selectedAgent = selectedNode?.kind === 'agent' ? selectedNode : undefined
  const selectedModelId = selectedAgent?.modelConfigId ?? null
  const modelOptions = [
    ...(selectedModelId && !enabledModels.some((m) => m.id === selectedModelId)
      ? [
          {
            id: selectedModelId,
            label: workflowNodeModelLabel(selectedAgent!, models, text),
            disabled: true
          }
        ]
      : []),
    ...enabledModels.map((m) => ({ id: m.id, label: formatModelConfigLabel(m) }))
  ]
  const select = (next: WorkflowSelection) => {
    setSelection(next)
    setConfiguredNodeId((current) => (current && next?.kind === 'node' ? next.id : null))
  }
  const configureNode = (id: string) => {
    setSelection({ kind: 'node', id })
    setConfiguredNodeId(id)
  }
  const closeInspector = () => {
    setConfiguredNodeId(null)
    window.requestAnimationFrame(() => {
      const node = canvasRef.current?.querySelector<HTMLElement>(
        selection?.kind === 'department'
          ? `[data-workflow-department-id="${CSS.escape(selection.id)}"] .workflow-department__heading`
          : `[data-workflow-node-id="${CSS.escape(selection?.id ?? '')}"]`
      )
      node?.focus({ preventScroll: true })
    })
  }
  const updateNode = (
    id: string,
    patch: Partial<WorkflowNode> | Partial<WorkflowAgentNode>,
    group?: string
  ) =>
    onChange(
      (current) => ({
        ...current,
        nodes: current.nodes.map((node) =>
          node.id === id ? ({ ...node, ...patch } as WorkflowNode) : node
        )
      }),
      { group }
    )
  const focusCanvas = () =>
    canvasRef.current?.querySelector<HTMLElement>('.workflow-canvas')?.focus()
  const deleteSelected = () => {
    if (selectedDepartment) {
      onChange((current) => removeWorkflowDepartment(current, selectedDepartment.id))
    } else if (selectedNode) {
      onChange((current) => ({
        ...current,
        nodes: current.nodes.filter((n) => n.id !== selectedNode.id)
      }))
    } else return
    setSelection(null)
    setConfiguredNodeId(null)
    focusCanvas()
  }
  const addNode = (x?: number, y?: number) => {
    if (graph.nodes.length >= 128) return
    const origin = canvasOrigin(graphBounds(graph))
    const center = {
      x:
        x ??
        (graph.viewport.x + (canvasRef.current?.clientWidth ?? 800) / 2) / graph.viewport.zoom -
          origin.x -
          106,
      y:
        y ??
        (graph.viewport.y + (canvasRef.current?.clientHeight ?? 600) / 2) / graph.viewport.zoom -
          origin.y -
          26
    }
    const position =
      x !== undefined && y !== undefined ? center : nearestOpenPosition(graph.nodes, center)
    const node = createWorkflowNode(
      text('newNode'),
      position.x,
      position.y,
      enabledModels[0]?.id ?? null,
      graph.nodes
    )
    onChange((current) =>
      normalizeWorkflowDepartments({ ...current, nodes: [...current.nodes, node] })
    )
    setDepartmentDrawing(false)
    setPanel(false)
    select({ kind: 'node', id: node.id })
    focusCanvas()
  }
  const selectionActions = (
    <div className="workflow-selection-actions">
      <Tooltip content={text('details')} preferredPlacement="bottom">
        <button
          type="button"
          className="workflow-graph-icon-button"
          aria-label={text('details')}
          onClick={() => selectedNode && configureNode(selectedNode.id)}
        >
          <Settings aria-hidden="true" />
        </button>
      </Tooltip>
      <Tooltip content={text('deleteNode')}>
        <button
          type="button"
          className="workflow-graph-icon-button workflow-selection-delete"
          aria-label={text('deleteNode')}
          onClick={deleteSelected}
        >
          <Trash2 aria-hidden="true" />
        </button>
      </Tooltip>
    </div>
  )
  return (
    <div
      className="workflow-graph-editor"
      inert={disabled || undefined}
      onCopy={(event) => {
        if (!selectedNode || !canUseNodeClipboard(event) || window.getSelection()?.toString())
          return
        const value = serializeWorkflowNode(selectedNode)
        event.clipboardData.setData(WORKFLOW_NODE_CLIPBOARD_TYPE, value)
        event.clipboardData.setData('text/plain', value)
        event.preventDefault()
      }}
      onPaste={(event) => {
        if (graph.nodes.length >= 128 || !canUseNodeClipboard(event)) return
        const source = readWorkflowNodeClipboard(
          event.clipboardData.getData(WORKFLOW_NODE_CLIPBOARD_TYPE) ||
            event.clipboardData.getData('text/plain')
        )
        if (!source || source.kind !== 'agent') return
        event.preventDefault()
        const node = duplicateWorkflowNode(
          source,
          nearestOpenPosition(graph.nodes, { x: source.x + 32, y: source.y + 32 }),
          graph.nodes
        )
        onChange((current) =>
          normalizeWorkflowDepartments({ ...current, nodes: [...current.nodes, node] })
        )
        select({ kind: 'node', id: node.id })
        focusCanvas()
      }}
      onKeyDown={(event) => {
        if (
          event.defaultPrevented ||
          (event.target as HTMLElement).closest('input, textarea, select, [contenteditable="true"]')
        )
          return
        if (event.key === 'Escape') {
          setPanel(false)
          setDepartmentDrawing(false)
          closeInspector()
          event.preventDefault()
        }
        if ((event.key === 'Delete' || event.key === 'Backspace') && selection) {
          event.preventDefault()
          deleteSelected()
        }
      }}
    >
      <div className="workflow-graph-editor__topbar">
        <div className="workflow-graph-toolbar" ref={toolbarRef}>
          <Tooltip content={text('addNode')} preferredPlacement="bottom">
            <button
              className="workflow-graph-toolbar__add"
              type="button"
              aria-label={text('addNode')}
              aria-expanded={panel}
              onClick={() => setPanel(!panel)}
            >
              <Plus aria-hidden="true" />
            </button>
          </Tooltip>
          {panel && (
            <div className="workflow-node-picker" role="dialog" aria-label={text('paletteTitle')}>
              <button
                className="workflow-node-picker__item"
                type="button"
                draggable
                disabled={graph.nodes.length >= 128}
                onDragStart={(event) => {
                  event.dataTransfer.setData(WORKFLOW_DRAG_TYPE, 'blank')
                  event.dataTransfer.effectAllowed = 'copy'
                }}
                onDragEnd={() => setPanel(false)}
                onClick={() => addNode()}
              >
                <span className="workflow-node-picker__icon">
                  <Plus aria-hidden="true" />
                </span>
                <strong>{text('blank')}</strong>
              </button>
              <button
                className="workflow-node-picker__item"
                type="button"
                disabled={(graph.departments?.length ?? 0) >= 64}
                onClick={() => {
                  setDepartmentDrawing(true)
                  setPanel(false)
                  focusCanvas()
                }}
              >
                <span className="workflow-node-picker__icon">
                  <SquareDashed aria-hidden="true" />
                </span>
                <strong>{text('addDepartment')}</strong>
              </button>
            </div>
          )}
        </div>
        {selectedAgent ? (
          <div
            className="workflow-selection-controls workflow-agent-controls"
            role="group"
            aria-label={text('quickSettings')}
          >
            <label className="workflow-selection-field">
              <AgentAvatar agentId={selectedAgent.id} className="workflow-selection-avatar" />
              <input
                aria-label={text('nodeName')}
                maxLength={128}
                value={selectedAgent.name}
                onChange={(event) =>
                  updateNode(
                    selectedAgent.id,
                    { name: event.currentTarget.value },
                    `node:${selectedAgent.id}:name`
                  )
                }
              />
            </label>
            <div className="workflow-selection-field">
              <span>{text('permission')}</span>
              <WorkflowOptionPicker
                ariaLabel={text('permission')}
                value={selectedAgent.permissionMode}
                options={CHAT_PERMISSION_PRESENTATIONS.map(({ id, labelKey, icon }) => ({
                  id,
                  name: t(labelKey),
                  icon
                }))}
                onChange={(permissionMode) => {
                  if (
                    permissionMode === 'default' ||
                    permissionMode === 'custom' ||
                    permissionMode === 'full'
                  )
                    updateNode(selectedAgent.id, { permissionMode })
                }}
              />
            </div>
            <label className="workflow-selection-field workflow-node-model">
              <span>{text('model')}</span>
              <ModelConfigPicker
                ariaLabel={text('selectModel')}
                className="workflow-node-model__picker"
                emptyLabel={text(enabledModels.length ? 'modelNotSelected' : 'noEnabledModels')}
                options={modelOptions}
                value={selectedModelId}
                onChange={(modelConfigId) => updateNode(selectedAgent.id, { modelConfigId })}
                variant="settings"
                portalMenu
                popoverClassName="workflow-node-model-popover"
              />
            </label>
            {selectionActions}
          </div>
        ) : null}
        {selectedDepartment ? (
          <div
            className="workflow-selection-controls workflow-department-controls"
            role="group"
            aria-label={text('departmentSettings')}
          >
            <div className="workflow-department-parent" aria-label={text('departmentParent')}>
              <span>{text('departmentParent')}</span>
              <span className="workflow-department-parent__name" title={parentDepartmentLabel}>
                {parentDepartmentLabel}
              </span>
            </div>
            <label className="workflow-selection-field">
              <Building2 aria-hidden="true" />
              <input
                aria-label={text('departmentName')}
                maxLength={128}
                value={selectedDepartment.name}
                onChange={(event) => {
                  const name = event.currentTarget.value
                  onChange(
                    (current) => ({
                      ...current,
                      departments: current.departments?.map((department) =>
                        department.id === selectedDepartment.id
                          ? { ...department, name }
                          : department
                      )
                    }),
                    { group: `department:${selectedDepartment.id}:name` }
                  )
                }}
              />
            </label>
            <Tooltip content={text('deleteDepartment')}>
              <button
                type="button"
                className="workflow-graph-icon-button workflow-selection-delete"
                aria-label={text('deleteDepartment')}
                onClick={deleteSelected}
              >
                <Trash2 aria-hidden="true" />
              </button>
            </Tooltip>
          </div>
        ) : null}
      </div>
      <div className="workflow-graph-editor__body">
        <div className="workflow-graph-editor__canvas" ref={canvasRef}>
          <WorkflowCanvas
            canvasActions={canvasActions}
            graph={graph}
            conversationBindings={conversationBindings}
            models={models}
            text={text}
            selection={selection}
            onChange={onChange}
            onSelect={select}
            onConfigureNode={configureNode}
            onAddNode={addNode}
            departmentDrawing={departmentDrawing}
            onDepartmentDrawingChange={setDepartmentDrawing}
          />
          {departmentDrawing && (
            <div className="workflow-department-drawing-hint" role="status">
              <SquareDashed aria-hidden="true" />
              <span>{text('drawDepartmentHint')}</span>
              <button
                className="workflow-graph-icon-button"
                type="button"
                aria-label={text('drawDepartmentCancel')}
                onClick={() => setDepartmentDrawing(false)}
              >
                <X aria-hidden="true" />
              </button>
            </div>
          )}
        </div>
        {configuredNode && (
          <aside className="workflow-graph-inspector" aria-label={text('nodeSettings')}>
            <div className="workflow-graph-inspector__header">
              <strong>{workflowNodeLabel(configuredNode, text)}</strong>
              <button
                className="workflow-graph-icon-button"
                type="button"
                aria-label={text('closeInspector')}
                onClick={closeInspector}
              >
                <X aria-hidden="true" />
              </button>
            </div>
            <div
              className="workflow-graph-inspector__tabs"
              role="tablist"
              aria-label={text('nodeSettings')}
            >
              {(['task', 'rank'] as const).map((tab) => (
                <button
                  key={tab}
                  type="button"
                  role="tab"
                  id={`${inspectorId}-${tab}-tab`}
                  aria-selected={inspectorTab === tab}
                  aria-controls={`${inspectorId}-panel`}
                  tabIndex={inspectorTab === tab ? 0 : -1}
                  onClick={() => setInspectorTab(tab)}
                  onKeyDown={(event) => {
                    if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
                      event.preventDefault()
                      const next =
                        event.key === 'Home'
                          ? 'task'
                          : event.key === 'End'
                            ? 'rank'
                            : tab === 'task'
                              ? 'rank'
                              : 'task'
                      setInspectorTab(next)
                      document.getElementById(`${inspectorId}-${next}-tab`)?.focus()
                    }
                  }}
                >
                  {text(tab === 'task' ? 'nodeTaskTab' : 'nodeRankTab')}
                </button>
              ))}
            </div>
            <div
              className="workflow-graph-inspector__body"
              key={configuredNode.id}
              role="tabpanel"
              id={`${inspectorId}-panel`}
              aria-labelledby={`${inspectorId}-${inspectorTab}-tab`}
            >
              {inspectorTab === 'rank' ? (
                <>
                  <label>
                    <span>{text('rank')}</span>
                    <input
                      aria-label={text('rank')}
                      type="number"
                      min={1}
                      max={99}
                      step={1}
                      value={configuredNode.rank ?? 1}
                      onChange={(event) => {
                        const rank = event.currentTarget.valueAsNumber
                        if (Number.isInteger(rank) && rank >= 1 && rank <= 99)
                          updateNode(configuredNode.id, { rank }, `node:${configuredNode.id}:rank`)
                      }}
                    />
                    <small className="workflow-inspector-hint">{text('rankHint')}</small>
                  </label>
                  <div className="workflow-template-field">
                    <span>{text('managementRole')}</span>
                    <WorkflowOptionPicker
                      ariaLabel={text('managementRole')}
                      value={configuredNode.managementRole ?? 'member'}
                      showSelectedDetail={false}
                      options={[
                        { id: 'member', name: text('roleMember') },
                        { id: 'organization_admin', name: text('roleOrganizationAdmin') },
                        {
                          id: 'department_admin',
                          name: text('roleDepartmentAdmin'),
                          disabled: !configuredNode.departmentId
                        }
                      ]}
                      onChange={(managementRole) => {
                        if (
                          managementRole === 'member' ||
                          managementRole === 'organization_admin' ||
                          (managementRole === 'department_admin' && configuredNode.departmentId)
                        )
                          updateNode(configuredNode.id, { managementRole })
                      }}
                    />
                    <p className="workflow-inspector-hint">
                      {text(
                        configuredNode.managementRole === 'organization_admin'
                          ? 'roleOrganizationAdminHint'
                          : configuredNode.managementRole === 'department_admin'
                            ? 'roleDepartmentAdminHint'
                            : 'roleMemberHint'
                      )}
                    </p>
                    {!configuredNode.departmentId &&
                      configuredNode.managementRole !== 'organization_admin' && (
                        <p className="workflow-inspector-hint">
                          {text('departmentAdminUnavailable')}
                        </p>
                      )}
                  </div>
                </>
              ) : (
                <>
                  {conversationBindings && (
                    <label>
                      <span>{conversationBindings.label}</span>
                      <SettingsSelect
                        ariaLabel={`${conversationBindings.label} · ${configuredNode.name}`}
                        value={conversationBindings.values[configuredNode.id] ?? ''}
                        options={[
                          { value: '', label: conversationBindings.emptyLabel },
                          ...conversationBindings.options.map((item) => ({
                            value: item.id,
                            label: item.title
                          }))
                        ]}
                        onChange={(id) =>
                          conversationBindings.onChange(configuredNode.id, id || null)
                        }
                      />
                    </label>
                  )}
                  <label>
                    <span>{text('receives')}</span>
                    <textarea
                      rows={3}
                      maxLength={32768}
                      value={configuredNode.receives}
                      onChange={(event) =>
                        updateNode(
                          configuredNode.id,
                          { receives: event.currentTarget.value },
                          `node:${configuredNode.id}:receives`
                        )
                      }
                    />
                  </label>
                  <label>
                    <span>{text('task')}</span>
                    <textarea
                      rows={6}
                      maxLength={32768}
                      value={configuredNode.task}
                      onChange={(event) =>
                        updateNode(
                          configuredNode.id,
                          { task: event.currentTarget.value },
                          `node:${configuredNode.id}:task`
                        )
                      }
                    />
                  </label>
                  <label>
                    <span>{text('delivers')}</span>
                    <textarea
                      rows={3}
                      maxLength={32768}
                      value={configuredNode.delivers}
                      onChange={(event) =>
                        updateNode(
                          configuredNode.id,
                          { delivers: event.currentTarget.value },
                          `node:${configuredNode.id}:delivers`
                        )
                      }
                    />
                  </label>
                </>
              )}
            </div>
          </aside>
        )}
      </div>
    </div>
  )
}
function canUseNodeClipboard(event: ClipboardEvent<HTMLDivElement>): boolean {
  const target = event.target
  return (
    !event.defaultPrevented &&
    target instanceof HTMLElement &&
    event.currentTarget.contains(target) &&
    !target.isContentEditable &&
    !target.closest('input, textarea, select, [inert], fieldset:disabled') &&
    !document.querySelector('[aria-modal="true"]')
  )
}

/** Click insertion searches nearby empty slots; explicit drop coordinates remain authoritative. */
function nearestOpenPosition(
  nodes: readonly { x: number; y: number; kind?: unknown }[],
  center: { x: number; y: number },
  dimensions = workflowNodeSize()
) {
  const candidates = new Map<string, { x: number; y: number; distance: number }>()
  const gap = 24
  for (let row = -32; row <= 32; row++) {
    for (let column = -32; column <= 32; column++) {
      const x = Math.max(24, Math.min(99000, center.x + column * (dimensions.width + gap)))
      const y = Math.max(70, Math.min(99000, center.y + row * (dimensions.height + gap)))
      candidates.set(`${x}:${y}`, { x, y, distance: (x - center.x) ** 2 + (y - center.y) ** 2 })
    }
  }
  const position = [...candidates.values()]
    .sort((left, right) => left.distance - right.distance || left.y - right.y || left.x - right.x)
    .find(
      (candidate) =>
        !nodes.some(
          (node) =>
            candidate.x <
              node.x + workflowNodeSize(node.kind ? { kind: node.kind } : undefined).width + gap &&
            candidate.x + dimensions.width + gap > node.x &&
            candidate.y <
              node.y + workflowNodeSize(node.kind ? { kind: node.kind } : undefined).height + gap &&
            candidate.y + dimensions.height + gap > node.y
        )
    )
  return position ?? center
}
