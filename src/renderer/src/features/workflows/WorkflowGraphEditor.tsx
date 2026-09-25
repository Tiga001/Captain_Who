import {
  ArrowDownToLine,
  ArrowUpFromLine,
  Bot,
  ChevronDown,
  Link2,
  Plus,
  Search,
  Trash2,
  X
} from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import type {
  AgentTemplate,
  WorkflowDefinition,
  WorkflowEndpoint,
  WorkflowFlow,
  WorkflowNode
} from '@mycopilot/protocol'
import {
  createWorkflowNode,
  endpointValue,
  nodeFlows,
  removeWorkflowFlows,
  workflowEndpoint,
  WORKFLOW_DRAG_TYPE,
  NODE_WIDTH,
  NODE_HEIGHT
} from './workflowAuthoring'
import { WorkflowCanvas, type WorkflowSelection } from './WorkflowCanvas'
import { WorkflowRuleEditor } from './WorkflowRuleEditor'
import { WorkflowTemplatePicker } from './WorkflowTemplatePicker'
import { canvasOrigin, graphBounds } from './workflowCanvasGeometry'
import type { WorkflowText } from './workflowText'
import { useModelSettings } from '../../config/ModelSettingsProvider'
import { ModelConfigPicker } from '../modelSelection/ModelConfigPicker'
import { formatModelConfigLabel } from '../modelSelection/modelConfigPresentation'
import { workflowNodeModelLabel, workflowTemplateModelLabel } from './workflowModelPresentation'
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
  const enabledModelIds = new Set(enabledModels.map((model) => model.id))
  const [selection, setSelection] = useState<WorkflowSelection>(null)
  const [configuredNodeId, setConfiguredNodeId] = useState<string | null>(null)
  const [pending, setPending] = useState<WorkflowEndpoint | null>(null)
  const pendingRef = useRef<WorkflowEndpoint | null>(null)
  const [panel, setPanel] = useState<'nodes' | 'connect' | null>(null)
  const [query, setQuery] = useState('')
  const [inspectorTab, setInspectorTab] = useState<'task' | 'rules'>('task')
  const [flowSource, setFlowSource] = useState('')
  const [flowTarget, setFlowTarget] = useState(graph.nodes[0]?.id ?? '')
  const toolbarRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLDivElement>(null)
  const addButtonRef = useRef<HTMLButtonElement>(null)
  const connectButtonRef = useRef<HTMLButtonElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const panelId = useId()
  const inspectorId = useId()
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

  const selectedTemplate = configuredNode?.templateId
    ? templates.find((template) => template.templateId === configuredNode.templateId)
    : undefined
  const selectedModelId = configuredNode?.modelConfigId ?? null
  const selectedModelUnavailable = Boolean(selectedModelId && !enabledModelIds.has(selectedModelId))
  const modelOptions = [
    ...(selectedModelUnavailable && selectedModelId
      ? [
          {
            id: selectedModelId,
            disabled: true,
            label: workflowNodeModelLabel(configuredNode!, templates, models, text)
          }
        ]
      : []),
    ...enabledModels.map((model) => ({ id: model.id, label: formatModelConfigLabel(model) }))
  ]

  useEffect(() => {
    if (!panel) return
    if (panel === 'nodes') searchRef.current?.focus()
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !toolbarRef.current?.contains(event.target))
        setPanel(null)
    }
    window.addEventListener('pointerdown', outside)
    return () => window.removeEventListener('pointerdown', outside)
  }, [panel])

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
    if (next?.kind !== 'node' || next.id !== configuredNodeId) setConfiguredNodeId(null)
  }
  const configureNode = (id: string) => {
    if (!graph.nodes.some((node) => node.id === id)) return
    setSelection({ kind: 'node', id })
    setConfiguredNodeId(id)
    setInspectorTab('task')
    setPanel(null)
  }
  const updateNode = (id: string, patch: Partial<WorkflowNode>, group?: string) =>
    onChange(
      (current) => ({
        ...current,
        nodes: current.nodes.map((node) => (node.id === id ? { ...node, ...patch } : node))
      }),
      group ? { group } : undefined
    )
  const addNode = (x?: number, y?: number, templateId?: string) => {
    if (graph.nodes.length >= 128) return
    const template = templateId
      ? templates.find((candidate) => candidate.templateId === templateId)
      : undefined
    if (templateId && !template) return
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
              NODE_WIDTH / 2
        )
      ),
      y: Math.max(
        70,
        Math.min(
          99000,
          y ??
            (graph.viewport.y + (area?.clientHeight ?? 600) / 2) / graph.viewport.zoom -
              origin.y -
              NODE_HEIGHT / 2
        )
      )
    }
    const position =
      x === undefined && y === undefined
        ? nearestOpenPosition(
            [...graph.nodes, graph.boundaryPositions.input, graph.boundaryPositions.output],
            desired
          )
        : desired
    const node = createWorkflowNode(
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
    setFlowTarget(node.id)
    setPanel(null)
  }
  const addFlow = (source: WorkflowEndpoint, target: WorkflowEndpoint) => {
    if (graph.flows.length >= 512 || (source.kind === 'boundary' && target.kind === 'boundary'))
      return
    const endpointExists = (endpoint: WorkflowEndpoint) =>
      endpoint.kind === 'boundary' || graph.nodes.some((node) => node.id === endpoint.nodeId)
    if (!endpointExists(source) || !endpointExists(target)) return
    const flow: WorkflowFlow = { id: crypto.randomUUID(), name: '', source, target }
    onChange((current) =>
      current.flows.length >= 512 ? current : { ...current, flows: [...current.flows, flow] }
    )
    select({ kind: 'flow', id: flow.id })
    setPanel(null)
  }
  const beginConnection = (source: WorkflowEndpoint) => {
    if (graph.flows.length >= 512) return
    pendingRef.current = source
    setPending(source)
    setPanel(null)
  }
  const cancelConnection = () => {
    pendingRef.current = null
    setPending(null)
  }
  const finishConnection = (target: WorkflowEndpoint) => {
    const source = pendingRef.current
    if (!source || (source.kind === 'boundary' && target.kind === 'boundary')) return
    cancelConnection()
    addFlow(source, target)
  }
  const focusCanvas = () =>
    canvasRef.current?.querySelector<HTMLElement>('.workflow-canvas')?.focus()
  const closeInspector = () => {
    if (configuredNodeId) {
      const id = configuredNodeId
      setConfiguredNodeId(null)
      window.requestAnimationFrame(() => {
        const gear = canvasRef.current?.querySelector<HTMLButtonElement>(
          `[data-configure-node="${CSS.escape(id)}"]`
        )
        if (gear) gear.focus({ preventScroll: true })
        else focusCanvas()
      })
    } else {
      select(null)
      focusCanvas()
    }
  }
  const deleteSelected = () => {
    if (selectedNode) {
      const id = selectedNode.id
      onChange((current) => {
        const ids = new Set(
          [...nodeFlows(current, id, 'input'), ...nodeFlows(current, id, 'output')].map(
            (flow) => flow.id
          )
        )
        const cleaned = removeWorkflowFlows(current, ids)
        return { ...cleaned, nodes: cleaned.nodes.filter((node) => node.id !== id) }
      })
      if (pendingRef.current?.kind === 'node' && pendingRef.current.nodeId === id)
        cancelConnection()
      if (flowSource === id) setFlowSource('')
      if (flowTarget === id) setFlowTarget('')
    } else if (selectedFlow)
      onChange((current) => removeWorkflowFlows(current, new Set([selectedFlow.id])))
    select(null)
    focusCanvas()
  }
  const updateFlow = (id: string, patch: Partial<WorkflowFlow>) =>
    onChange(
      (current) => {
        const previous = current.flows.find((flow) => flow.id === id)
        if (!previous) return current
        const changed = { ...previous, ...patch }
        const flows = current.flows.map((flow) => (flow.id === id ? changed : flow))
        if (!patch.source && !patch.target) return { ...current, flows }
        const nodes = current.nodes.map((node) => {
          const clean = (rule: WorkflowNode['inputRule'], endpoint: WorkflowEndpoint) => {
            if (endpoint.kind === 'node' && endpoint.nodeId === node.id) return rule
            return {
              ...rule,
              required: rule.required.filter((member) => member !== id),
              groups: rule.groups
                .map((group) => ({
                  ...group,
                  flowIds: group.flowIds.filter((member) => member !== id)
                }))
                .filter((group) => group.flowIds.length > 0)
            }
          }
          return {
            ...node,
            inputRule: clean(node.inputRule, changed.target),
            outputRule: clean(node.outputRule, changed.source)
          }
        })
        return { ...current, flows, nodes }
      },
      patch.name !== undefined ? { group: `flow:${id}:name` } : undefined
    )
  const flowLabel = (flow: WorkflowFlow) => {
    if (flow.name) return flow.name
    const name = (endpoint: WorkflowEndpoint, entry: boolean) =>
      endpoint.kind === 'boundary'
        ? text(entry ? 'parentInput' : 'parentOutput')
        : graph.nodes.find((node) => node.id === endpoint.nodeId)?.name || text('newNode')
    return `${name(flow.source, true)} → ${name(flow.target, false)}`
  }
  const nodeOptions = graph.nodes.map((node) => (
    <option key={node.id} value={node.id}>
      {node.name || text('newNode')}
    </option>
  ))
  const sourceValue = graph.nodes.some((node) => node.id === flowSource) ? flowSource : ''
  const targetValue = graph.nodes.some((node) => node.id === flowTarget) ? flowTarget : ''

  return (
    <div
      className="workflow-graph-editor"
      onKeyDown={(event) => {
        if (event.defaultPrevented) return
        if (event.key === 'Escape') {
          if (panel) {
            const trigger = panel === 'nodes' ? addButtonRef.current : connectButtonRef.current
            setPanel(null)
            trigger?.focus()
          } else if (pendingRef.current) cancelConnection()
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
      <div className="workflow-graph-editor__canvas" ref={canvasRef}>
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
          <button
            ref={connectButtonRef}
            className="workflow-graph-icon-button"
            type="button"
            title={text('connectNodes')}
            aria-label={text('connectNodes')}
            aria-expanded={panel === 'connect'}
            aria-controls={panel === 'connect' ? panelId : undefined}
            disabled={graph.nodes.length === 0 || graph.flows.length >= 512}
            onClick={() => {
              const source = selectedNode?.id ?? sourceValue
              setFlowSource(source)
              setFlowTarget(
                targetValue && targetValue !== source
                  ? targetValue
                  : (graph.nodes.find((node) => node.id !== source)?.id ?? '')
              )
              setPanel(panel === 'connect' ? null : 'connect')
            }}
          >
            <Link2 aria-hidden="true" />
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
              <div className="workflow-node-picker__heading">{text('templates')}</div>
              <div className="workflow-node-picker__list">
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
          ) : panel === 'connect' ? (
            <div
              className="workflow-connect-picker"
              id={panelId}
              role="dialog"
              aria-label={text('connectNodes')}
            >
              <label>
                <span>{text('source')}</span>
                <select
                  aria-label={`${text('addFlow')} ${text('source')}`}
                  value={sourceValue}
                  onChange={(event) => setFlowSource(event.currentTarget.value)}
                >
                  <option value="">{text('parentInput')}</option>
                  {nodeOptions}
                </select>
              </label>
              <label>
                <span>{text('target')}</span>
                <select
                  aria-label={`${text('addFlow')} ${text('target')}`}
                  value={targetValue}
                  onChange={(event) => setFlowTarget(event.currentTarget.value)}
                >
                  <option value="">{text('parentOutput')}</option>
                  {nodeOptions}
                </select>
              </label>
              <button
                className="workflow-button workflow-button--wide"
                type="button"
                disabled={!sourceValue && !targetValue}
                onClick={() =>
                  addFlow(workflowEndpoint(sourceValue), workflowEndpoint(targetValue))
                }
              >
                {text('addFlow')}
              </button>
            </div>
          ) : null}
        </div>
        {pending ? (
          <div className="workflow-graph-connection-status" role="status">
            <span>{text(pending.kind === 'boundary' ? 'connectingEntry' : 'connecting')}</span>
            {pending.kind === 'node' ? (
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
          onChange={onChange}
          onSelect={select}
          onConfigureNode={configureNode}
          onAddNode={addNode}
          onBegin={beginConnection}
          onFinish={finishConnection}
        />
      </div>
      {configuredNode || selectedFlow ? (
        <aside className="workflow-graph-inspector" aria-label={text('nodeSettings')}>
          <div className="workflow-graph-inspector__header">
            <strong>
              {configuredNode?.name ||
                (configuredNode ? text('newNode') : selectedFlow?.name || text('connection'))}
            </strong>
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
              <div
                className="workflow-graph-inspector__tabs"
                role="tablist"
                aria-label={text('nodeSettings')}
              >
                {(['task', 'rules'] as const).map((tab) => (
                  <button
                    key={tab}
                    type="button"
                    role="tab"
                    aria-selected={inspectorTab === tab}
                    aria-controls={`${inspectorId}-${tab}`}
                    id={`${inspectorId}-tab-${tab}`}
                    onClick={() => setInspectorTab(tab)}
                    onKeyDown={(event) => {
                      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
                      event.preventDefault()
                      const next =
                        event.key === 'Home'
                          ? 'task'
                          : event.key === 'End'
                            ? 'rules'
                            : tab === 'task'
                              ? 'rules'
                              : 'task'
                      setInspectorTab(next)
                      document.getElementById(`${inspectorId}-tab-${next}`)?.focus()
                    }}
                  >
                    {text(tab === 'task' ? 'nodeTaskTab' : 'nodeRulesTab')}
                  </button>
                ))}
              </div>
              <div
                className="workflow-graph-inspector__body"
                role="tabpanel"
                id={`${inspectorId}-${inspectorTab}`}
                aria-labelledby={`${inspectorId}-tab-${inspectorTab}`}
                key={`${configuredNode.id}-${inspectorTab}`}
              >
                {inspectorTab === 'task' ? (
                  <>
                    <label>
                      <span>{text('nodeName')}</span>
                      <input
                        maxLength={128}
                        value={configuredNode.name}
                        onChange={(event) =>
                          updateNode(
                            configuredNode.id,
                            { name: event.currentTarget.value },
                            `node:${configuredNode.id}:name`
                          )
                        }
                      />
                    </label>
                    <div className="workflow-template-field">
                      <span>{text('template')}</span>
                      <WorkflowTemplatePicker
                        value={configuredNode.templateId}
                        templates={templates}
                        models={models}
                        text={text}
                        onChange={(templateId) => {
                          if (templateId === configuredNode.templateId) return
                          const template = templates.find(
                            (candidate) => candidate.templateId === templateId
                          )
                          updateNode(configuredNode.id, {
                            templateId: template?.templateId ?? null,
                            modelConfigId: template
                              ? null
                              : selectedTemplate &&
                                  enabledModelIds.has(selectedTemplate.modelConfigId)
                                ? selectedTemplate.modelConfigId
                                : (enabledModels[0]?.id ?? null),
                            ...(!configuredNode.task && template
                              ? { task: template.instructions.slice(0, 32768) }
                              : {})
                          })
                        }}
                      />
                    </div>
                    {configuredNode.templateId &&
                    !templates.some(
                      (template) => template.templateId === configuredNode.templateId
                    ) ? (
                      <p className="workflow-error" role="alert">
                        {text('missingTemplate')}
                      </p>
                    ) : null}
                    <label className="workflow-node-model">
                      <span>{text('model')}</span>
                      {configuredNode.templateId ? (
                        <input
                          readOnly
                          value={workflowNodeModelLabel(configuredNode, templates, models, text)}
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
                          emptyLabel={text(
                            enabledModels.length ? 'modelNotSelected' : 'noEnabledModels'
                          )}
                          options={modelOptions}
                          value={selectedModelId}
                          onChange={(modelConfigId) =>
                            updateNode(configuredNode.id, { modelConfigId })
                          }
                          variant="settings"
                          portalMenu
                          popoverClassName="workflow-node-model-popover"
                        />
                      )}
                    </label>
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
                ) : (
                  <>
                    <WorkflowRuleEditor
                      direction="input"
                      flows={incoming}
                      rule={configuredNode.inputRule}
                      text={text}
                      flowLabel={flowLabel}
                      onChange={(inputRule) => updateNode(configuredNode.id, { inputRule })}
                    />
                    <WorkflowRuleEditor
                      direction="output"
                      flows={outgoing}
                      rule={configuredNode.outputRule}
                      text={text}
                      flowLabel={flowLabel}
                      onChange={(outputRule) => updateNode(configuredNode.id, { outputRule })}
                    />
                  </>
                )}
              </div>
              <div className="workflow-graph-inspector__footer">
                <button
                  type="button"
                  className="workflow-graph-icon-button"
                  title={text('addEntry')}
                  aria-label={text('addEntry')}
                  disabled={graph.flows.length >= 512}
                  onClick={() =>
                    addFlow({ kind: 'boundary' }, { kind: 'node', nodeId: configuredNode.id })
                  }
                >
                  <ArrowDownToLine aria-hidden="true" />
                </button>
                <button
                  type="button"
                  className="workflow-graph-icon-button"
                  title={text('addExit')}
                  aria-label={text('addExit')}
                  disabled={graph.flows.length >= 512}
                  onClick={() =>
                    addFlow({ kind: 'node', nodeId: configuredNode.id }, { kind: 'boundary' })
                  }
                >
                  <ArrowUpFromLine aria-hidden="true" />
                </button>
                <button
                  type="button"
                  className="workflow-graph-icon-button workflow-graph-inspector__delete"
                  aria-label={text('deleteNode')}
                  onClick={deleteSelected}
                >
                  <Trash2 aria-hidden="true" />
                </button>
              </div>
            </>
          ) : selectedFlow ? (
            <>
              <div className="workflow-graph-inspector__body">
                <label>
                  <span>{text('flowName')}</span>
                  <input
                    maxLength={128}
                    value={selectedFlow.name}
                    onChange={(event) =>
                      updateFlow(selectedFlow.id, { name: event.currentTarget.value })
                    }
                  />
                </label>
                <label>
                  <span>{text('source')}</span>
                  <select
                    value={endpointValue(selectedFlow.source)}
                    onChange={(event) =>
                      updateFlow(selectedFlow.id, {
                        source: workflowEndpoint(event.currentTarget.value)
                      })
                    }
                  >
                    <option value="" disabled={selectedFlow.target.kind === 'boundary'}>
                      {text('parentInput')}
                    </option>
                    {nodeOptions}
                  </select>
                </label>
                <label>
                  <span>{text('target')}</span>
                  <select
                    value={endpointValue(selectedFlow.target)}
                    onChange={(event) =>
                      updateFlow(selectedFlow.id, {
                        target: workflowEndpoint(event.currentTarget.value)
                      })
                    }
                  >
                    <option value="" disabled={selectedFlow.source.kind === 'boundary'}>
                      {text('parentOutput')}
                    </option>
                    {nodeOptions}
                  </select>
                </label>
              </div>
              <div className="workflow-graph-inspector__footer">
                <button
                  type="button"
                  className="workflow-graph-icon-button workflow-graph-inspector__delete"
                  aria-label={text('deleteFlow')}
                  onClick={deleteSelected}
                >
                  <Trash2 aria-hidden="true" />
                </button>
              </div>
            </>
          ) : null}
        </aside>
      ) : null}
    </div>
  )
}

/** Click insertion searches nearby empty slots; explicit drop coordinates remain authoritative. */
function nearestOpenPosition(
  nodes: readonly { x: number; y: number }[],
  center: { x: number; y: number }
) {
  const candidates = new Map<string, { x: number; y: number; distance: number }>()
  const gap = 24
  for (let row = -32; row <= 32; row++) {
    for (let column = -32; column <= 32; column++) {
      const x = Math.max(24, Math.min(99000, center.x + column * (NODE_WIDTH + gap)))
      const y = Math.max(70, Math.min(99000, center.y + row * (NODE_HEIGHT + gap)))
      candidates.set(`${x}:${y}`, { x, y, distance: (x - center.x) ** 2 + (y - center.y) ** 2 })
    }
  }
  const position = [...candidates.values()]
    .sort((left, right) => left.distance - right.distance || left.y - right.y || left.x - right.x)
    .find(
      (candidate) =>
        !nodes.some(
          (node) =>
            candidate.x < node.x + NODE_WIDTH + gap &&
            candidate.x + NODE_WIDTH + gap > node.x &&
            candidate.y < node.y + NODE_HEIGHT + gap &&
            candidate.y + NODE_HEIGHT + gap > node.y
        )
    )
  return position ?? center
}
