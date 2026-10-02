import { Tooltip } from '../../components/overlay/Tooltip'
import { AccountAvatar } from '../auth/AccountAvatar'
import { useAccountAuth } from '../auth/AccountAuthContext'
import { UserRound, Plus, Settings, Trash2, X } from 'lucide-react'
import { useEffect, useRef, useState, type ClipboardEvent } from 'react'
import type { WorkflowDefinition, WorkflowNode, WorkflowAgentNode } from '@mycopilot/protocol'
import {
  createWorkflowNode,
  createWorkflowUser,
  workflowNodeSize,
  WORKFLOW_DRAG_TYPE
} from './workflowAuthoring'
import { WorkflowCanvas, type WorkflowSelection } from './WorkflowCanvas'
import {
  WORKFLOW_NODE_CLIPBOARD_TYPE,
  serializeWorkflowNode,
  readWorkflowNodeClipboard,
  duplicateWorkflowNode
} from './workflowNodeClipboard'
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
import './workflowGraphEditor.css'
export interface WorkflowGraphEditorProps {
  definition: WorkflowDefinition
  text: WorkflowText
  onChange: (
    update: (current: WorkflowDefinition) => WorkflowDefinition,
    options?: { group?: string; transient?: boolean }
  ) => void
}
export function WorkflowGraphEditor({
  definition: graph,
  text,
  onChange
}: WorkflowGraphEditorProps) {
  const { t } = useFrontendConfig()
  const { models, enabledModels } = useModelSettings()
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || text('currentUser')
  const [selection, setSelection] = useState<WorkflowSelection>(null)
  const [configuredNodeId, setConfiguredNodeId] = useState<string | null>(null)
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
  const selectedNode = graph.nodes.find((n) => n.id === selection?.id)
  const configuredNode = graph.nodes.find((n) => n.id === configuredNodeId)
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
    setConfiguredNodeId((current) => (current ? (next?.id ?? null) : null))
  }
  const configureNode = (id: string) => {
    setSelection({ kind: 'node', id })
    setConfiguredNodeId(id)
  }
  const closeInspector = () => {
    setConfiguredNodeId(null)
    window.requestAnimationFrame(() => {
      const node = canvasRef.current?.querySelector<HTMLElement>(
        `[data-workflow-node-id="${CSS.escape(selection?.id ?? '')}"]`
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
    if (!selectedNode) return
    onChange((current) => ({
      ...current,
      nodes: current.nodes.filter((n) => n.id !== selectedNode.id)
    }))
    setSelection(null)
    setConfiguredNodeId(null)
    focusCanvas()
  }
  const addNode = (x?: number, y?: number, kind?: string) => {
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
    const node =
      kind === 'node:user'
        ? createWorkflowUser(userName, position.x, position.y)
        : createWorkflowNode(text('newNode'), position.x, position.y, enabledModels[0]?.id ?? null)
    onChange((current) => ({ ...current, nodes: [...current.nodes, node] }))
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
        if (!source) return
        event.preventDefault()
        const node = duplicateWorkflowNode(
          source,
          nearestOpenPosition(graph.nodes, { x: source.x + 32, y: source.y + 32 })
        )
        onChange((current) => ({ ...current, nodes: [...current.nodes, node] }))
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
          closeInspector()
          event.preventDefault()
        }
        if ((event.key === 'Delete' || event.key === 'Backspace') && selectedNode) {
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
              disabled={graph.nodes.length >= 128}
              onClick={() => setPanel(!panel)}
            >
              <Plus aria-hidden="true" />
            </button>
          </Tooltip>
          {panel && (
            <div className="workflow-node-picker" role="dialog" aria-label={text('paletteTitle')}>
              {[
                { kind: 'node:user', label: text('user'), icon: UserRound },
                { kind: 'blank', label: text('blank'), icon: Plus }
              ].map(({ kind, label, icon: Icon }) => (
                <button
                  key={kind}
                  className="workflow-node-picker__item"
                  type="button"
                  draggable
                  onDragStart={(event) => {
                    event.dataTransfer.setData(WORKFLOW_DRAG_TYPE, kind)
                    event.dataTransfer.effectAllowed = 'copy'
                  }}
                  onDragEnd={() => setPanel(false)}
                  onClick={() => addNode(undefined, undefined, kind)}
                >
                  <span className="workflow-node-picker__icon">
                    <Icon aria-hidden="true" />
                  </span>
                  <strong>{label}</strong>
                </button>
              ))}
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
        {selectedNode?.kind === 'user' ? (
          <div
            className="workflow-selection-controls workflow-user-controls"
            role="group"
            aria-label={text('user')}
          >
            <div className="workflow-selection-field">
              <span className="workflow-selection-avatar workflow-user-avatar">
                <AccountAvatar
                  src={profile?.avatarDataUrl}
                  localAvatarSeed={profile?.localAccount?.avatarSeed}
                />
              </span>
              <input aria-label={text('user')} readOnly value={userName} />
            </div>
            {selectionActions}
          </div>
        ) : null}
      </div>
      <div className="workflow-graph-editor__body">
        <div className="workflow-graph-editor__canvas" ref={canvasRef}>
          <WorkflowCanvas
            graph={graph}
            models={models}
            text={text}
            selection={selection}
            onChange={onChange}
            onSelect={select}
            onConfigureNode={configureNode}
            onAddNode={addNode}
          />
        </div>
        {configuredNode && (
          <aside className="workflow-graph-inspector" aria-label={text('nodeSettings')}>
            <div className="workflow-graph-inspector__header">
              <strong>{workflowNodeLabel(configuredNode, text, userName)}</strong>
              <button
                className="workflow-graph-icon-button"
                type="button"
                aria-label={text('closeInspector')}
                onClick={closeInspector}
              >
                <X aria-hidden="true" />
              </button>
            </div>
            <div className="workflow-graph-inspector__body" key={configuredNode.id}>
              {configuredNode.kind === 'user' ? (
                <label>
                  <span>{text('userTask')}</span>
                  <textarea
                    rows={6}
                    maxLength={32768}
                    placeholder={text('userTaskPlaceholder')}
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
              ) : (
                <>
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
