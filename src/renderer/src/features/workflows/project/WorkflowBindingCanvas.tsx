import type { WorkflowDefinition } from '@mycopilot/protocol'
import { Maximize2, Minus, Plus } from 'lucide-react'
import { useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { dismissActiveTooltip, Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { AgentAvatar } from '../../agentCollaboration/AgentAvatar'
import { AccountAvatar } from '../../auth/AccountAvatar'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import { getChatPermissionPresentation } from '../../chat/chatPermissionPresentation'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import { workflowNodeSize } from '../workflowAuthoring'
import { graphBounds, graphFlowLayout } from '../workflowCanvasGeometry'
import { workflowNodeModelLabel, type WorkflowModelDisplay } from '../workflowModelPresentation'
import { workflowText } from '../workflowText'
import { projectWorkflowText, WORKFLOW_CONVERSATION_DRAG_TYPE } from './projectWorkflowText'
import '../workflowCanvas.css'
import './workflowBindingCanvas.css'

interface Props {
  graph: WorkflowDefinition
  conversations: readonly ChatConversation[]
  conversationDrafts?: Readonly<Record<string, ChatComposerDraft>>
  models: readonly WorkflowModelDisplay[]
  bindings: Readonly<Record<string, string | null>>
  savedBindings: Readonly<Record<string, string>>
  color: string
  selectedNodeId: string | null
  disabled?: boolean
  onSelect: (nodeId: string) => void
  onBind: (nodeId: string, conversationId: string) => void
}

/** A binding surface: positions and connections always come from the selected template. */
export function WorkflowBindingCanvas({
  graph,
  conversations,
  conversationDrafts,
  models,
  bindings,
  savedBindings,
  color,
  selectedNodeId,
  disabled = false,
  onSelect,
  onBind
}: Props) {
  const { language } = useFrontendConfig()
  const text = workflowText(language)
  const t = projectWorkflowText(language)
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || t('当前用户', 'Current user')
  const viewportRef = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ width: 900, height: 550 })
  const [manualZoom, setManualZoom] = useState<number | null>(null)
  const [dragTarget, setDragTarget] = useState<string | null>(null)
  const pan = useRef<{ x: number; y: number; left: number; top: number } | null>(null)
  const markerId = useId().replaceAll(':', '')
  const { geometries } = useMemo(() => graphFlowLayout(graph), [graph])
  const bounds = useMemo(() => graphBounds(graph, geometries), [graph, geometries])
  const origin = { x: 64 - bounds.left, y: 64 - bounds.top }
  const width = Math.max(400, bounds.right - bounds.left + 128)
  const height = Math.max(260, bounds.bottom - bounds.top + 128)
  const fitZoom = Math.min(1, Math.max(0.15, Math.min(size.width / width, size.height / height)))
  const zoom = manualZoom ?? fitZoom

  useLayoutEffect(() => {
    const element = viewportRef.current
    if (!element) return
    const observer = new ResizeObserver(() => {
      setSize({ width: element.clientWidth || 900, height: element.clientHeight || 550 })
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  const conversationModel = (conversation: ChatConversation) => {
    const modelId = conversationDrafts?.[conversation.id]?.modelId ?? conversation.modelId
    const model = models.find((item) => item.id === modelId)
    return model ? formatModelConfigLabel(model) : t('未选择模型', 'No model selected')
  }

  return (
    <div
      className="workflow-canvas-shell workflow-binding-canvas-shell"
      style={{ '--workflow-instance-color': color } as CSSProperties}
    >
      <div
        ref={viewportRef}
        className="workflow-binding-canvas"
        aria-label={t('工作流对话绑定画布', 'Workflow conversation binding canvas')}
        onPointerDownCapture={() => dismissActiveTooltip()}
        onPointerDown={(event) => {
          if (event.button !== 0 || (event.target as Element).closest('button')) return
          pan.current = {
            x: event.clientX,
            y: event.clientY,
            left: event.currentTarget.scrollLeft,
            top: event.currentTarget.scrollTop
          }
          event.currentTarget.setPointerCapture(event.pointerId)
        }}
        onPointerMove={(event) => {
          if (!pan.current) return
          event.currentTarget.scrollLeft = pan.current.left - event.clientX + pan.current.x
          event.currentTarget.scrollTop = pan.current.top - event.clientY + pan.current.y
        }}
        onPointerUp={() => (pan.current = null)}
        onPointerCancel={() => (pan.current = null)}
      >
        <div
          className="workflow-binding-canvas__extent"
          style={{
            width: Math.max(size.width, width * zoom),
            height: Math.max(size.height, height * zoom)
          }}
        >
          <div
            className="workflow-canvas__stage workflow-binding-canvas__stage"
            style={{
              width,
              height,
              left: Math.max(0, (size.width - width * zoom) / 2),
              top: Math.max(0, (size.height - height * zoom) / 2),
              transform: `scale(${zoom})`
            }}
          >
            <svg
              className="workflow-canvas__edges"
              width={width}
              height={height}
              aria-hidden="true"
            >
              <defs>
                <marker
                  id={markerId}
                  markerWidth="7"
                  markerHeight="7"
                  refX="6.2"
                  refY="3.5"
                  orient="auto"
                >
                  <path d="M 0 0 L 7 3.5 L 0 7 z" fill="currentColor" />
                </marker>
              </defs>
              <g transform={`translate(${origin.x} ${origin.y})`}>
                {graph.flows.map((flow) => {
                  const geometry = geometries.get(flow.id)
                  return geometry ? (
                    <g key={flow.id} className="workflow-edge">
                      <path
                        className="workflow-edge__line"
                        d={geometry.path}
                        markerEnd={`url(#${markerId})`}
                      />
                      <text
                        className="workflow-edge__label"
                        x={geometry.label.x}
                        y={geometry.label.y - 7}
                        textAnchor="middle"
                      >
                        {flow.name}
                      </text>
                    </g>
                  ) : null
                })}
                {graph.flows.flatMap((flow) =>
                  (geometries.get(flow.id)?.bridges ?? []).map((bridge, index) => (
                    <g key={`${flow.id}-bridge-${index}`} className="workflow-crossing">
                      <path className="workflow-crossing__halo" d={bridge.path} />
                      <path className="workflow-crossing__line" d={bridge.path} />
                    </g>
                  ))
                )}
              </g>
            </svg>
            <div
              className="workflow-node workflow-node--root"
              style={{
                left: graph.boundaryPositions.input.x + origin.x,
                top: graph.boundaryPositions.input.y + origin.y,
                ...workflowNodeSize()
              }}
            >
              <span className="workflow-node__avatar workflow-user-avatar">
                <AccountAvatar src={profile?.avatarDataUrl} />
              </span>
              <div className="workflow-node__copy">
                <strong>{userName}</strong>
                <span>{t('用户输入', 'User input')}</span>
              </div>
            </div>
            {graph.nodes.map((node) => {
              const dimensions = workflowNodeSize(node)
              const style = {
                left: node.x + origin.x,
                top: node.y + origin.y,
                width: dimensions.width,
                height: dimensions.height
              }
              if (node.kind === 'inputGate' || node.kind === 'outputGate') {
                return (
                  <div
                    key={node.id}
                    className={`workflow-node workflow-node--gate workflow-node--${node.kind}`}
                    style={style}
                    title={node.name}
                  >
                    <svg className="workflow-gate-shape" viewBox="0 0 64 56" aria-hidden="true">
                      <path
                        d={
                          node.kind === 'inputGate'
                            ? 'M 2 2 L 62 28 L 2 54 Z'
                            : 'M 62 2 L 2 28 L 62 54 Z'
                        }
                      />
                    </svg>
                  </div>
                )
              }
              if (node.kind === 'user') {
                return (
                  <div key={node.id} className="workflow-node" style={style}>
                    <span className="workflow-node__avatar workflow-user-avatar">
                      <AccountAvatar src={profile?.avatarDataUrl} />
                    </span>
                    <div className="workflow-node__copy">
                      <strong>{userName}</strong>
                      <span>{t('用户', 'User')}</span>
                    </div>
                  </div>
                )
              }
              const conversation = conversations.find((item) => item.id === bindings[node.id])
              const keepsConversationSettings =
                !!conversation && conversation.id === savedBindings[node.id]
              const conversationDraft = conversation
                ? conversationDrafts?.[conversation.id]
                : undefined
              const permissionMode =
                keepsConversationSettings && conversationDraft
                  ? conversationDraft.permissionMode
                  : node.permissionMode
              const PermissionIcon = getChatPermissionPresentation(permissionMode).icon
              const tooltip = (
                <div className="workflow-binding-node-tooltip">
                  <strong>{node.name}</strong>
                  <dl>
                    <div>
                      <dt>
                        {conversation && keepsConversationSettings
                          ? t('当前模型', 'Current model')
                          : t('节点模型', 'Node model')}
                      </dt>
                      <dd>
                        {conversation && keepsConversationSettings
                          ? conversationModel(conversation)
                          : workflowNodeModelLabel(node, models, text)}
                      </dd>
                    </div>
                    <div>
                      <dt>
                        {keepsConversationSettings && conversationDraft
                          ? t('当前权限', 'Current permissions')
                          : t('节点权限', 'Node permissions')}
                      </dt>
                      <dd
                        className="workflow-binding-node-tooltip__permission"
                        data-permission={permissionMode}
                      >
                        <PermissionIcon />
                        {permissionMode === 'full'
                          ? t('完全权限', 'Full permissions')
                          : permissionMode === 'custom'
                            ? t('自定义权限', 'Custom permissions')
                            : t('默认权限', 'Default permissions')}
                      </dd>
                    </div>
                    <div className="workflow-binding-node-tooltip__task">
                      <dt>{t('任务', 'Task')}</dt>
                      <dd>{node.task || t('未填写任务说明', 'No task instructions')}</dd>
                    </div>
                  </dl>
                </div>
              )
              return (
                <div key={node.id} className="workflow-binding-node-anchor" style={style}>
                  <Tooltip
                    content={tooltip}
                    delayMs={1000}
                    delayOnFocus
                    describeTrigger
                    anchorClassName="workflow-binding-node-tooltip-anchor"
                  >
                    <button
                      type="button"
                      className={`workflow-node workflow-binding-node${selectedNodeId === node.id ? ' is-selected' : ''}${dragTarget === node.id ? ' is-drop-target' : ''}${conversation ? ' is-bound' : ''}`}
                      aria-label={`${t('绑定对话', 'Bind conversation')} · ${node.name}`}
                      aria-pressed={selectedNodeId === node.id}
                      onClick={() => onSelect(node.id)}
                      onDragOver={(event) => {
                        dismissActiveTooltip()
                        if (
                          disabled ||
                          !event.dataTransfer.types.includes(WORKFLOW_CONVERSATION_DRAG_TYPE)
                        )
                          return
                        event.preventDefault()
                        event.dataTransfer.dropEffect = 'link'
                        setDragTarget(node.id)
                      }}
                      onDragLeave={() => setDragTarget(null)}
                      onDrop={(event) => {
                        event.preventDefault()
                        dismissActiveTooltip()
                        setDragTarget(null)
                        const id = event.dataTransfer.getData(WORKFLOW_CONVERSATION_DRAG_TYPE)
                        if (id && !disabled) onBind(node.id, id)
                      }}
                    >
                      <AgentAvatar agentId={node.id} className="workflow-node__avatar" />
                      <span className="workflow-node__copy workflow-node__copy--configurable">
                        <strong>{conversation?.title || node.name}</strong>
                        <span>
                          {conversation
                            ? node.name
                            : t('未绑定 · 确认后新建', 'Unbound · creates on confirm')}
                        </span>
                      </span>
                    </button>
                  </Tooltip>
                </div>
              )
            })}
          </div>
        </div>
      </div>
      <div className="workflow-canvas-actions" onPointerDownCapture={() => dismissActiveTooltip()}>
        <div className="workflow-canvas-controls" role="group" aria-label={text('resetView')}>
          <button
            type="button"
            title={text('zoomOut')}
            aria-label={text('zoomOut')}
            disabled={zoom <= 0.25}
            onClick={() => setManualZoom(Math.max(0.25, zoom - 0.1))}
          >
            <Minus size={14} />
          </button>
          <button
            type="button"
            className="workflow-canvas-controls__percentage"
            title="100%"
            aria-label="100%"
            onClick={() => setManualZoom(1)}
          >
            {Math.round(zoom * 100)}%
          </button>
          <button
            type="button"
            title={text('zoomIn')}
            aria-label={text('zoomIn')}
            disabled={zoom >= 2}
            onClick={() => setManualZoom(Math.min(2, zoom + 0.1))}
          >
            <Plus size={14} />
          </button>
          <span className="workflow-canvas-controls__separator" />
          <button
            type="button"
            title={text('resetView')}
            aria-label={text('resetView')}
            onClick={() => {
              setManualZoom(null)
              viewportRef.current?.scrollTo(0, 0)
            }}
          >
            <Maximize2 size={14} />
          </button>
        </div>
      </div>
    </div>
  )
}
