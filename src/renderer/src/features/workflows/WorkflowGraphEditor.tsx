import { AccountAvatar } from '../auth/AccountAvatar'
import { useAccountAuth } from '../auth/AccountAuthContext'
import {
  Bot,
  UserRound,
  Triangle,
  ChevronDown,
  Link2,
  Hand,
  Plus,
  Search,
  Settings,
  Trash2,
  X
} from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import type {
  AgentTemplate,
  WorkflowDefinition,
  WorkflowEndpoint,
  WorkflowAnchor,
  WorkflowFlow,
  WorkflowNode,
  WorkflowAgentNode,
  WorkflowUserNode,
  WorkflowInputGateNode,
  WorkflowOutputGateNode
} from '@mycopilot/protocol'
import {
  createWorkflowNode,
  createWorkflowUser,
  createWorkflowGate,
  workflowNodeSize,
  connectWorkflowFlow,
  workflowConnectionAllowed,
  endpointValue,
  nodeFlows,
  removeWorkflowFlows,
  workflowEndpoint,
  WORKFLOW_DRAG_TYPE
} from './workflowAuthoring'
import { WorkflowCanvas, type WorkflowSelection } from './WorkflowCanvas'
import { AgentAvatar } from '../agentCollaboration/AgentAvatar'
import { WorkflowGateEditor } from './WorkflowGateEditor'
import { WorkflowTemplatePicker } from './WorkflowTemplatePicker'
import { WorkflowOptionPicker } from './WorkflowOptionPicker'
import { canvasOrigin, graphBounds } from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import { useModelSettings } from '../../config/ModelSettingsProvider'
import { ModelConfigPicker } from '../modelSelection/ModelConfigPicker'
import { formatModelConfigLabel } from '../modelSelection/modelConfigPresentation'
import {
  workflowNodeModelLabel,
  workflowNodeLabel,
  workflowTemplateModelLabel
} from './workflowModelPresentation'
import './workflowGraphEditor.css'

export interface WorkflowGraphEditorProps {
  definition: WorkflowDefinition
  templates: readonly AgentTemplate[]
  text: WorkflowText
  onChange: (
    update: (current: WorkflowDefinition) => WorkflowDefinition,
    options?: { group?: string; transient?: boolean }
  ) => void
}

export function WorkflowGraphEditor({
  definition: graph,
  templates,
  text,
  onChange
}: WorkflowGraphEditorProps) {
  const { models, enabledModels } = useModelSettings()
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || text('currentUser')
  const enabledModelIds = new Set(enabledModels.map((model) => model.id))
  const [selection, setSelection] = useState<WorkflowSelection>(null)
  const [configuredNodeId, setConfiguredNodeId] = useState<string | null>(null)
  const [pending, setPending] = useState<WorkflowEndpoint | null>(null)
  const pendingRef = useRef<WorkflowEndpoint | null>(null)
  const [panel, setPanel] = useState<'nodes' | null>(null)
  const [connectionMode, setConnectionMode] = useState(false)
  const [pendingAnchor, setPendingAnchor] = useState<WorkflowAnchor | undefined>()
  const pendingAnchorRef = useRef<WorkflowAnchor | undefined>(undefined)
  const [query, setQuery] = useState('')
  const toolbarRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLDivElement>(null)
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const connectButtonRef = useRef<HTMLButtonElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const panelId = useId()
  const selectedNode =
    selection?.kind === 'node' ? graph.nodes.find((node) => node.id === selection.id) : undefined
  const selectedFlow =
    selection?.kind === 'flow' ? graph.flows.find((flow) => flow.id === selection.id) : undefined
  const configuredNode = graph.nodes.find((node) => node.id === configuredNodeId)
  const incoming = configuredNode ? nodeFlows(graph, configuredNode.id, 'input') : []
  const outgoing = configuredNode ? nodeFlows(graph, configuredNode.id, 'output') : []
  const matchingTemplates = templates.filter((template) =>
    `${template.name} ${template.description} ${workflowTemplateModelLabel(template, models, text)}`
      .toLocaleLowerCase()
      .includes(query.trim().toLocaleLowerCase())
  )

  const selectedAgent = selectedNode?.kind === 'agent' ? selectedNode : undefined
  const selectedTemplate = selectedAgent?.templateId
    ? templates.find((template) => template.templateId === selectedAgent.templateId)
    : undefined
  const selectedModelId = selectedAgent?.modelConfigId ?? null
  const selectedModelUnavailable = Boolean(selectedModelId && !enabledModelIds.has(selectedModelId))
  const modelOptions = [
    ...(selectedModelUnavailable && selectedModelId
      ? [
          {
            id: selectedModelId,
            disabled: true,
            label: workflowNodeModelLabel(selectedAgent!, templates, models, text)
          }
        ]
      : []),
    ...enabledModels.map((model) => ({ id: model.id, label: formatModelConfigLabel(model) }))
  ]

  useEffect(() => {
    if (!panel) return
    if (panel === 'nodes') searchRef.current?.focus()
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !toolbarRef.current?.contains(event.target) &&
        !(
          event.target instanceof Element &&
          event.target.closest(`[data-workflow-picker-owner="${CSS.escape(panelId)}"]`)
        )
      )
        setPanel(null)
    }
    window.addEventListener('pointerdown', outside)
    return () => window.removeEventListener('pointerdown', outside)
  }, [panel, panelId])

  useEffect(() => {
    if (pending?.kind === 'node' && !graph.nodes.some((node) => node.id === pending.nodeId)) {
      pendingRef.current = null
      setPending(null)
    }
  }, [graph.nodes, pending])

  useEffect(() => {
    if (configuredNodeId && !graph.nodes.some((node) => node.id === configuredNodeId))
      setConfiguredNodeId(null)
    if (
      (selection?.kind === 'node' && !graph.nodes.some((node) => node.id === selection.id)) ||
      (selection?.kind === 'flow' && !graph.flows.some((flow) => flow.id === selection.id))
    )
      setSelection(null)
  }, [configuredNodeId, graph.flows, graph.nodes, selection])

  const select = (next: WorkflowSelection) => {
    setSelection(next)
    // An open inspector follows node selection without collapsing or resizing the canvas.
    // A closed inspector still requires the explicit configure action.
    if (next?.kind !== 'node') setConfiguredNodeId(null)
    else if (!connectionMode) setConfiguredNodeId((current) => (current ? next.id : null))
  }
  const configureNode = (id: string) => {
    if (!graph.nodes.some((node) => node.id === id)) return
    setSelection({ kind: 'node', id })
    setConfiguredNodeId(id)
    setPanel(null)
  }
  const updateNode = (
    id: string,
    patch:
      | Partial<WorkflowAgentNode>
      | Partial<WorkflowUserNode>
      | Partial<WorkflowInputGateNode>
      | Partial<WorkflowOutputGateNode>,
    group?: string
  ) =>
    onChange(
      (current) => ({
        ...current,
        nodes: current.nodes.map((node) =>
          node.id === id ? ({ ...node, ...patch } as WorkflowNode) : node
        )
      }),
      group ? { group } : undefined
    )
  const addNode = (x?: number, y?: number, templateId?: string) => {
    const userNode = templateId === 'node:user'
    const gateKind =
      templateId === 'gate:input' ? 'inputGate' : templateId === 'gate:output' ? 'outputGate' : null
    if (graph.nodes.length >= 128) return
    const template =
      templateId && !gateKind
        ? templates.find((candidate) => candidate.templateId === templateId)
        : undefined
    if (templateId && !gateKind && !userNode && !template) return
    const dimensions = workflowNodeSize(gateKind ? { kind: gateKind } : undefined)
    const area = canvasRef.current
    const origin = canvasOrigin(graphBounds(graph))
    const desired = {
      x: Math.max(
        24,
        Math.min(
          99000,
          x ??
            (graph.viewport.x + (area?.clientWidth ?? 800) / 2) / graph.viewport.zoom -
              origin.x -
              dimensions.width / 2
        )
      ),
      y: Math.max(
        70,
        Math.min(
          99000,
          y ??
            (graph.viewport.y + (area?.clientHeight ?? 600) / 2) / graph.viewport.zoom -
              origin.y -
              dimensions.height / 2
        )
      )
    }
    const position =
      x === undefined && y === undefined
        ? nearestOpenPosition(
            [...graph.nodes, graph.boundaryPositions.input, graph.boundaryPositions.output],
            desired,
            workflowNodeSize(gateKind ? { kind: gateKind } : undefined)
          )
        : desired
    const node = userNode
      ? createWorkflowUser(userName, position.x, position.y)
      : gateKind
        ? createWorkflowGate(gateKind, position.x, position.y, graph.nodes)
        : createWorkflowNode(
            text('newNode'),
            position.x,
            position.y,
            template,
            template ? null : (enabledModels[0]?.id ?? null)
          )
    onChange((current) =>
      current.nodes.length >= 128 ? current : { ...current, nodes: [...current.nodes, node] }
    )
    select({ kind: 'node', id: node.id })
    setConfiguredNodeId(null)
    setPanel(null)
  }
  const addFlow = (
    source: WorkflowEndpoint,
    target: WorkflowEndpoint,
    sourceAnchor?: WorkflowAnchor,
    targetAnchor?: WorkflowAnchor
  ) => {
    const flow: WorkflowFlow = {
      id: crypto.randomUUID(),
      name: '',
      source,
      target,
      ...(sourceAnchor ? { sourceAnchor } : {}),
      ...(targetAnchor ? { targetAnchor } : {})
    }
    if (!workflowConnectionAllowed(graph, source, target)) return
    onChange((current) => connectWorkflowFlow(current, flow))
    select({ kind: 'flow', id: flow.id })
    setPanel(null)
  }
  const beginConnection = (source: WorkflowEndpoint, anchor?: WorkflowAnchor) => {
    if (graph.flows.length >= 512) return
    pendingAnchorRef.current = anchor
    setPendingAnchor(anchor)
    pendingRef.current = source
    setPending(source)
    setPanel(null)
  }
  const cancelConnection = () => {
    pendingRef.current = null
    setPending(null)
  }
  const finishConnection = (target: WorkflowEndpoint, anchor?: WorkflowAnchor) => {
    const source = pendingRef.current
    if (!source || !workflowConnectionAllowed(graph, source, target)) return
    cancelConnection()
    addFlow(source, target, pendingAnchorRef.current, anchor)
  }
  const focusCanvas = () =>
    canvasRef.current?.querySelector<HTMLElement>('.workflow-canvas')?.focus()
  const closeInspector = () => {
    if (configuredNodeId) {
      const id = configuredNodeId
      setConfiguredNodeId(null)
      window.requestAnimationFrame(() => {
        const node = canvasRef.current?.querySelector<HTMLElement>(
          `.workflow-node[data-workflow-node-id="${CSS.escape(id)}"]`
        )
        if (node) node.focus({ preventScroll: true })
        else focusCanvas()
      })
    } else {
      select(null)
      focusCanvas()
    }
  }
  const deleteNode = (id: string) => {
    onChange((current) => {
      const ids = new Set(
        [...nodeFlows(current, id, 'input'), ...nodeFlows(current, id, 'output')].map(
          (flow) => flow.id
        )
      )
      const cleaned = removeWorkflowFlows(current, ids)
      return { ...cleaned, nodes: cleaned.nodes.filter((node) => node.id !== id) }
    })
    if (pendingRef.current?.kind === 'node' && pendingRef.current.nodeId === id) cancelConnection()
    select(null)
    focusCanvas()
  }
  const deleteSelected = () => {
    if (selectedNode) deleteNode(selectedNode.id)
    else if (selectedFlow) {
      onChange((current) => removeWorkflowFlows(current, new Set([selectedFlow.id])))
      select(null)
      focusCanvas()
    }
  }
  const updateFlow = (id: string, patch: Partial<WorkflowFlow>) =>
    onChange(
      (current) => {
        const previous = current.flows.find((flow) => flow.id === id)
        if (!previous) return current
        const changed = { ...previous, ...patch }
        if (!patch.source && !patch.target)
          return {
            ...current,
            flows: current.flows.map((flow) => (flow.id === id ? changed : flow))
          }
        return connectWorkflowFlow(current, changed)
      },
      patch.name !== undefined ? { group: `flow:${id}:name` } : undefined
    )
  const flowLabel = (flow: WorkflowFlow) => {
    const name = (endpoint: WorkflowEndpoint, entry: boolean) =>
      endpoint.kind === 'boundary'
        ? text(entry ? 'parentInput' : 'parentOutput')
        : workflowNodeLabel(
            graph.nodes.find((node) => node.id === endpoint.nodeId)!,
            text,
            userName
          )
    return `${flow.name ? `${flow.name} · ` : ''}${name(flow.source, true)} → ${name(flow.target, false)}`
  }
  const nodeOptions = graph.nodes.map((node) => ({
    id: node.id,
    name: workflowNodeLabel(node, text, userName),
    detail: workflowNodeModelLabel(node, templates, models, text)
  }))

  const selectionActions = (
    <div className="workflow-selection-actions">
      {selectedNode ? (
        <button
          type="button"
          className="workflow-graph-toolbar__add"
          onClick={() => configureNode(selectedNode.id)}
        >
          <Settings aria-hidden="true" />
          {text('details')}
        </button>
      ) : null}
      <button
        type="button"
        className="workflow-graph-icon-button workflow-selection-delete"
        aria-label={text(selectedFlow ? 'deleteFlow' : 'deleteNode')}
        title={text(selectedFlow ? 'deleteFlow' : 'deleteNode')}
        onClick={deleteSelected}
      >
        <Trash2 aria-hidden="true" />
      </button>
    </div>
  )

  return (
    <div
      className="workflow-graph-editor"
      onKeyDown={(event) => {
        if (event.defaultPrevented) return
        if (event.key === 'Escape') {
          if (panel) {
            const trigger = addButtonRef.current
            setPanel(null)
            trigger?.focus()
          } else if (pendingRef.current) cancelConnection()
          else if (connectionMode) setConnectionMode(false)
          else if (configuredNodeId || selectedFlow) closeInspector()
          else if (selection) select(null)
          else return
          event.preventDefault()
          event.stopPropagation()
        }
        if (event.key !== 'Delete' && event.key !== 'Backspace') return
        if (
          (event.target as HTMLElement).closest(
            'input, textarea, select, button, [contenteditable="true"]'
          )
        )
          return
        if (!selectedNode && !selectedFlow) return
        event.preventDefault()
        deleteSelected()
      }}
    >
      <div className="workflow-graph-editor__topbar">
        <div className="workflow-graph-toolbar" ref={toolbarRef}>
          <button
            ref={addButtonRef}
            className="workflow-graph-toolbar__add"
            type="button"
            aria-expanded={panel === 'nodes'}
            aria-controls={panel === 'nodes' ? panelId : undefined}
            disabled={graph.nodes.length >= 128}
            onClick={() => {
              setPanel(panel === 'nodes' ? null : 'nodes')
              setQuery('')
            }}
          >
            <Plus aria-hidden="true" />
            {text('addNode')}
            <ChevronDown aria-hidden="true" />
          </button>
          <span className="workflow-graph-toolbar__divider" aria-hidden="true" />
          <button
            ref={connectButtonRef}
            className="workflow-graph-toolbar__add workflow-graph-toolbar__connect"
            type="button"
            title={text('connectNodes')}
            aria-label={text('connectNodes')}
            aria-pressed={connectionMode}
            disabled={graph.nodes.length === 0 || graph.flows.length >= 512}
            onClick={() => {
              setConnectionMode(true)
              setPanel(null)
              select(null)
              cancelConnection()
            }}
          >
            <Link2 aria-hidden="true" />
            {text('connection')}
          </button>
          <button
            type="button"
            className="workflow-graph-icon-button"
            title={text('operateMode')}
            aria-label={text('operateMode')}
            aria-pressed={!connectionMode}
            onClick={() => {
              setConnectionMode(false)
              cancelConnection()
              setPanel(null)
            }}
          >
            <Hand aria-hidden="true" />
          </button>
          {panel === 'nodes' ? (
            <div
              className="workflow-node-picker"
              id={panelId}
              role="dialog"
              aria-label={text('paletteTitle')}
            >
              <label className="workflow-node-picker__search">
                <Search aria-hidden="true" />
                <input
                  ref={searchRef}
                  aria-label={text('searchTemplates')}
                  placeholder={text('searchTemplates')}
                  value={query}
                  onChange={(event) => setQuery(event.currentTarget.value)}
                />
              </label>
              <button
                className="workflow-node-picker__item"
                type="button"
                draggable
                onDragStart={(event) => {
                  event.dataTransfer.setData(WORKFLOW_DRAG_TYPE, 'node:user')
                  event.dataTransfer.effectAllowed = 'copy'
                }}
                onDragEnd={() => setPanel(null)}
                onClick={() => addNode(undefined, undefined, 'node:user')}
              >
                <span className="workflow-node-picker__icon">
                  <UserRound aria-hidden="true" />
                </span>
                <strong>{text('user')}</strong>
              </button>
              <div className="workflow-node-picker__heading">{text('logicGates')}</div>
              {(['inputGate', 'outputGate'] as const)
                .filter((kind) =>
                  text(kind).toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())
                )
                .map((kind) => (
                  <button
                    key={kind}
                    className="workflow-node-picker__item"
                    type="button"
                    draggable
                    onDragStart={(event) => {
                      event.dataTransfer.setData(
                        WORKFLOW_DRAG_TYPE,
                        kind === 'inputGate' ? 'gate:input' : 'gate:output'
                      )
                      event.dataTransfer.effectAllowed = 'copy'
                    }}
                    onDragEnd={() => setPanel(null)}
                    onClick={() =>
                      addNode(
                        undefined,
                        undefined,
                        kind === 'inputGate' ? 'gate:input' : 'gate:output'
                      )
                    }
                  >
                    <span className="workflow-node-picker__icon">
                      <Triangle
                        style={{
                          transform: kind === 'inputGate' ? 'rotate(90deg)' : 'rotate(-90deg)'
                        }}
                        aria-hidden="true"
                      />
                    </span>
                    <span>
                      <strong>{text(kind)}</strong>
                      <small>
                        {text(kind === 'inputGate' ? 'inputGateShort' : 'outputGateShort')}
                      </small>
                    </span>
                  </button>
                ))}
              <div className="workflow-node-picker__heading">{text('templates')}</div>
              <div className="workflow-node-picker__list">
                <button
                  className="workflow-node-picker__item"
                  type="button"
                  draggable
                  onDragStart={(event) => {
                    event.dataTransfer.setData(WORKFLOW_DRAG_TYPE, 'blank')
                    event.dataTransfer.effectAllowed = 'copy'
                  }}
                  onDragEnd={() => setPanel(null)}
                  onClick={() => addNode()}
                >
                  <span className="workflow-node-picker__icon">
                    <Plus aria-hidden="true" />
                  </span>
                  <strong>{text('blank')}</strong>
                </button>
                {matchingTemplates.map((template) => (
                  <button
                    key={template.templateId}
                    className="workflow-node-picker__item"
                    type="button"
                    draggable
                    onDragStart={(event) => {
                      event.dataTransfer.setData(WORKFLOW_DRAG_TYPE, template.templateId)
                      event.dataTransfer.effectAllowed = 'copy'
                    }}
                    onDragEnd={() => setPanel(null)}
                    onClick={() => addNode(undefined, undefined, template.templateId)}
                  >
                    <span className="workflow-node-picker__icon">
                      <Bot aria-hidden="true" />
                    </span>
                    <span>
                      <strong>{template.name}</strong>
                      <small>{workflowTemplateModelLabel(template, models, text)}</small>
                    </span>
                  </button>
                ))}
                {matchingTemplates.length === 0 ? (
                  <p className="workflow-node-picker__empty">
                    {text(templates.length ? 'noMatchingTemplates' : 'templateEmpty')}
                  </p>
                ) : null}
              </div>
            </div>
          ) : null}
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
            <div className="workflow-selection-field workflow-template-field">
              <span>{text('templateShort')}</span>
              <WorkflowTemplatePicker
                showSelectedDetail={false}
                value={selectedAgent.templateId}
                templates={templates}
                models={models}
                text={text}
                onChange={(templateId) => {
                  if (templateId === selectedAgent.templateId) return
                  const template = templates.find(
                    (candidate) => candidate.templateId === templateId
                  )
                  updateNode(selectedAgent.id, {
                    templateId: template?.templateId ?? null,
                    modelConfigId: template
                      ? null
                      : selectedTemplate && enabledModelIds.has(selectedTemplate.modelConfigId)
                        ? selectedTemplate.modelConfigId
                        : (enabledModels[0]?.id ?? null),
                    ...(!selectedAgent.task && template
                      ? { task: template.instructions.slice(0, 32768) }
                      : {})
                  })
                }}
              />
            </div>
            {selectedAgent.templateId &&
            !templates.some((template) => template.templateId === selectedAgent.templateId) ? (
              <p className="workflow-error" role="alert">
                {text('missingTemplate')}
              </p>
            ) : null}
            <label className="workflow-selection-field workflow-node-model">
              <span>{text('model')}</span>
              {selectedAgent.templateId ? (
                <input
                  readOnly
                  value={workflowNodeModelLabel(selectedAgent, templates, models, text)}
                  data-unavailable={
                    !selectedTemplate ||
                    !enabledModelIds.has(selectedTemplate.modelConfigId) ||
                    undefined
                  }
                />
              ) : (
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
              )}
            </label>
            {selectionActions}
          </div>
        ) : null}
        {selectedNode &&
        (selectedNode.kind === 'inputGate' || selectedNode.kind === 'outputGate') ? (
          <div
            className="workflow-selection-controls workflow-gate-controls"
            role="group"
            aria-label={text('gateSettings')}
          >
            <label className="workflow-selection-field">
              <span>{text('gateType')}</span>
              <input readOnly value={text(selectedNode.kind)} />
            </label>
            <label className="workflow-selection-field">
              <span>{text('nodeName')}</span>
              <input
                maxLength={128}
                value={selectedNode.name}
                onChange={(event) =>
                  updateNode(
                    selectedNode.id,
                    { name: event.currentTarget.value },
                    `node:${selectedNode.id}:name`
                  )
                }
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
                <AccountAvatar src={profile?.avatarDataUrl} />
              </span>
              <input aria-label={text('user')} readOnly value={userName} />
            </div>
            {selectionActions}
          </div>
        ) : null}
        {selectedFlow ? (
          <div
            className="workflow-selection-controls workflow-edge-controls"
            role="group"
            aria-label={text('connection')}
          >
            <label className="workflow-selection-field workflow-flow-name">
              <span>{text('flowName')}</span>
              <input
                maxLength={128}
                value={selectedFlow.name}
                onChange={(event) =>
                  updateFlow(selectedFlow.id, { name: event.currentTarget.value })
                }
              />
            </label>
            <div className="workflow-selection-field workflow-endpoint-field">
              <span>{text('source')}</span>
              <WorkflowOptionPicker
                showSelectedDetail={false}
                ariaLabel={text('source')}
                value={endpointValue(selectedFlow.source)}
                onChange={(id) =>
                  updateFlow(selectedFlow.id, {
                    source: workflowEndpoint(id ?? ''),
                    sourceAnchor: undefined
                  })
                }
                options={[
                  {
                    id: '',
                    name: text('parentInput'),
                    disabled: selectedFlow.target.kind === 'boundary'
                  },
                  ...nodeOptions
                ].map((option) => ({
                  ...option,
                  disabled: !workflowConnectionAllowed(
                    graph,
                    workflowEndpoint(option.id),
                    selectedFlow.target,
                    selectedFlow.id
                  )
                }))}
              />
            </div>
            <div className="workflow-selection-field workflow-endpoint-field">
              <span>{text('target')}</span>
              <WorkflowOptionPicker
                showSelectedDetail={false}
                ariaLabel={text('target')}
                value={endpointValue(selectedFlow.target)}
                onChange={(id) =>
                  updateFlow(selectedFlow.id, {
                    target: workflowEndpoint(id ?? ''),
                    targetAnchor: undefined
                  })
                }
                options={[
                  {
                    id: '',
                    name: text('parentOutput'),
                    disabled: selectedFlow.source.kind === 'boundary'
                  },
                  ...nodeOptions
                ].map((option) => ({
                  ...option,
                  disabled: !workflowConnectionAllowed(
                    graph,
                    selectedFlow.source,
                    workflowEndpoint(option.id),
                    selectedFlow.id
                  )
                }))}
              />
            </div>
            {selectionActions}
          </div>
        ) : null}
      </div>
      <div className="workflow-graph-editor__body">
        <div className="workflow-graph-editor__canvas" ref={canvasRef}>
          {pending ? (
            <div className="workflow-graph-connection-status" role="status">
              <span>{text(pending.kind === 'boundary' ? 'connectingEntry' : 'connecting')}</span>
              {pending.kind === 'node' &&
              workflowConnectionAllowed(graph, pending, { kind: 'boundary' }) ? (
                <button type="button" onClick={() => finishConnection({ kind: 'boundary' })}>
                  {text('addExit')}
                </button>
              ) : null}
              <button
                type="button"
                className="workflow-graph-icon-button"
                aria-label={text('cancelConnection')}
                onClick={cancelConnection}
              >
                <X aria-hidden="true" />
              </button>
            </div>
          ) : null}
          <WorkflowCanvas
            graph={graph}
            templates={templates}
            models={models}
            text={text}
            selection={selection}
            pending={pending}
            pendingAnchor={pendingAnchor}
            connectionMode={connectionMode}
            onChange={onChange}
            onSelect={select}
            onConfigureNode={configureNode}
            onAddNode={addNode}
            onBegin={beginConnection}
            onFinish={finishConnection}
          />
        </div>
        {configuredNode ? (
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
            {configuredNode ? (
              <>
                <div className="workflow-graph-inspector__body" key={configuredNode.id}>
                  {configuredNode.kind === 'inputGate' || configuredNode.kind === 'outputGate' ? (
                    <WorkflowGateEditor
                      node={configuredNode}
                      flows={configuredNode.kind === 'inputGate' ? incoming : outgoing}
                      text={text}
                      flowLabel={flowLabel}
                      recipient={
                        outgoing.some(
                          (flow) =>
                            flow.target.kind === 'node' &&
                            graph.nodes.some(
                              (node) =>
                                node.id === (flow.target as { nodeId: string }).nodeId &&
                                node.kind === 'user'
                            )
                        )
                          ? 'user'
                          : 'agent'
                      }
                      onChange={(node) => updateNode(configuredNode.id, node)}
                    />
                  ) : configuredNode.kind === 'user' ? (
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
                      {incoming.length ? (
                        <label>
                          <span>
                            {text(
                              incoming.every((flow) => flow.source.kind === 'boundary')
                                ? 'initialInput'
                                : 'receives'
                            )}
                          </span>
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
                      ) : null}
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
                      {outgoing.length ? (
                        <label>
                          <span>
                            {text(
                              outgoing.every((flow) => flow.target.kind === 'boundary')
                                ? 'finalOutput'
                                : 'delivers'
                            )}
                          </span>
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
                      ) : null}
                    </>
                  )}
                </div>
              </>
            ) : null}
          </aside>
        ) : null}
      </div>
    </div>
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
