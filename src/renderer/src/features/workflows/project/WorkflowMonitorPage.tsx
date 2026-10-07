import { WorkflowTransmissionLayer } from './WorkflowTransmissionLayer'
import { WorkflowNodeMailbox } from './WorkflowNodeMailbox'
import type { WorkflowInstance } from '@mycopilot/protocol'
import { ArrowLeft, Maximize2, Minus, Plus } from 'lucide-react'
import { useMemo, type CSSProperties } from 'react'
import { dismissActiveTooltip, Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { AgentAvatar } from '../../agentCollaboration/AgentAvatar'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import type { ConversationAttentionById } from '../../chat/useConversationAttention'
import { getChatPermissionPresentation } from '../../chat/chatPermissionPresentation'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import { workflowNodeSize } from '../workflowAuthoring'
import { workflowDepartmentLevel } from '../workflowDepartments'
import { graphBounds } from '../workflowCanvasGeometry'
import { workflowNodeModelLabel, type WorkflowModelDisplay } from '../workflowModelPresentation'
import { workflowText } from '../workflowText'
import { projectWorkflowText } from './projectWorkflowText'
import { useWorkflowMonitor } from './useWorkflowMonitor'
import { useWorkflowExecution } from './useWorkflowExecution'
import { useWorkflowMonitorViewport } from './useWorkflowMonitorViewport'
import '../workflowCanvas.css'
import './workflowBindingCanvas.css'
import './workflowMonitor.css'

interface Props {
  foreground?: boolean
  instance: WorkflowInstance
  conversations: readonly ChatConversation[]
  conversationDrafts?: Readonly<Record<string, ChatComposerDraft>>
  conversationAttention?: ConversationAttentionById
  models: readonly WorkflowModelDisplay[]
  onBack: () => void
  onOpenConversation: (id: string) => void
}

/** Read-only member layout; temporary connections come only from actual mail events. */
export function WorkflowMonitorPage({
  foreground = true,
  instance,
  conversations,
  conversationDrafts,
  conversationAttention,
  models,
  onBack,
  onOpenConversation
}: Props) {
  const graph = instance.definition
  const { language } = useFrontendConfig()
  const text = workflowText(language)
  const t = projectWorkflowText(language)
  const { snapshot, transmissions } = useWorkflowExecution(instance.id, foreground)
  const { runningConversationIds, waitingApprovalConversationIds } = useWorkflowMonitor(
    instance,
    conversations,
    foreground
  )
  const bounds = useMemo(() => graphBounds(graph), [graph])
  const bindings = useMemo(
    () => new Map(instance.bindings.map((binding) => [binding.nodeId, binding.conversationId])),
    [instance.bindings]
  )
  const conversationById = useMemo(
    () => new Map(conversations.map((conversation) => [conversation.id, conversation])),
    [conversations]
  )
  const pendingMailByNode = useMemo(() => {
    const counts = new Map<string, number>()
    for (const input of snapshot?.inputs ?? []) {
      if (input.mailStatus === 'pending')
        counts.set(input.nodeId, (counts.get(input.nodeId) ?? 0) + 1)
    }
    return counts
  }, [snapshot?.inputs])
  const modelById = useMemo(() => new Map(models.map((model) => [model.id, model])), [models])
  const departments = useMemo(
    () =>
      [...(graph.departments ?? [])]
        .sort((a, b) => b.width * b.height - a.width * a.height)
        .map((department) => ({
          ...department,
          level: workflowDepartmentLevel(graph, department.id)
        })),
    [graph]
  )
  const origin = { x: 64 - bounds.left, y: 64 - bounds.top }
  const width = Math.max(400, bounds.right - bounds.left + 128)
  const height = Math.max(260, bounds.bottom - bounds.top + 128)
  const { viewportRef, size, zoom, left, top, dragging, zoomTo, resetView, panHandlers } =
    useWorkflowMonitorViewport(width, height)

  return (
    <section
      className="workflow-monitor"
      aria-label={t('组织看板', 'Organization board')}
      style={{ '--workflow-instance-color': instance.color } as CSSProperties}
    >
      <header className="workflow-monitor__header">
        <Tooltip content={t('返回组织', 'Back to organizations')}>
          <button
            type="button"
            className="workflow-monitor__back"
            aria-label={t('返回组织', 'Back to organizations')}
            onClick={onBack}
          >
            <ArrowLeft size={18} />
          </button>
        </Tooltip>
        <span className="workflow-monitor__color" aria-hidden="true" />
        <h1>{instance.name}</h1>
      </header>
      <div className="workflow-monitor__body">
        <div className="workflow-canvas-shell workflow-binding-canvas-shell workflow-monitor__shell">
          <div
            ref={viewportRef}
            className={`workflow-binding-canvas workflow-monitor__canvas${dragging ? ' is-panning' : ''}`}
            aria-label={t('组织只读画布', 'Read-only organization canvas')}
            onPointerDownCapture={() => dismissActiveTooltip()}
            {...panHandlers}
          >
            <div
              className="workflow-binding-canvas__extent"
              style={{
                width: size.width,
                height: size.height
              }}
            >
              <div
                className="workflow-canvas__stage workflow-binding-canvas__stage"
                style={{
                  width,
                  height,
                  left,
                  top,
                  transform: `scale(${zoom})`
                }}
              >
                {departments.map((department) => (
                  <div
                    key={department.id}
                    className="workflow-department workflow-monitor__department"
                    data-workflow-department-id={department.id}
                    aria-label={`${department.name} · ${text('departmentLevel').replace('{level}', String(department.level))}`}
                    style={{
                      left: department.x + origin.x,
                      top: department.y + origin.y,
                      width: department.width,
                      height: department.height
                    }}
                  >
                    <div className="workflow-department__heading">
                      <strong>{department.name}</strong>
                      <span>
                        {text('departmentLevel').replace('{level}', String(department.level))}
                      </span>
                    </div>
                  </div>
                ))}
                <WorkflowTransmissionLayer
                  nodes={graph.nodes}
                  events={transmissions}
                  origin={origin}
                  width={width}
                  height={height}
                />
                {graph.nodes.map((node) => {
                  const dimensions = workflowNodeSize(node)
                  const style = {
                    left: node.x + origin.x,
                    top: node.y + origin.y,
                    width: dimensions.width,
                    height: dimensions.height
                  }
                  const conversationId = bindings.get(node.id)
                  const conversation = conversationId
                    ? conversationById.get(conversationId)
                    : undefined
                  const running = !!conversationId && runningConversationIds.has(conversationId)
                  const attention = conversationId
                    ? conversationAttention?.[conversationId]
                    : undefined
                  const waitingApproval =
                    !!conversationId &&
                    (!!waitingApprovalConversationIds?.has(conversationId) ||
                      (attention?.waitingApproval ??
                        conversation?.messages.some(
                          (message) =>
                            message.role === 'assistant' &&
                            message.agentRun?.status === 'waiting_for_approval'
                        )))
                  const waitingAnswer =
                    attention?.waitingAnswer ??
                    conversation?.messages.some(
                      (message) =>
                        message.role === 'assistant' &&
                        message.agentRun?.status === 'waiting_for_user_input'
                    )
                  const unread = attention?.unread ?? !!conversation?.unreadAt
                  const waitingLabel = waitingApproval
                    ? t('等待批准', 'Waiting for approval')
                    : waitingAnswer
                      ? t('等待交互', 'Waiting for interaction')
                      : null
                  const isAdmin =
                    node.managementRole === 'organization_admin' ||
                    node.managementRole === 'department_admin'
                  const draft = conversationId ? conversationDrafts?.[conversationId] : undefined
                  const modelId = draft?.modelId ?? conversation?.modelId
                  const model = modelId ? modelById.get(modelId) : undefined
                  const permission = draft?.permissionMode ?? node.permissionMode
                  const PermissionIcon = getChatPermissionPresentation(permission).icon
                  const tooltip = (
                    <div className="workflow-binding-node-tooltip">
                      <dl>
                        <div>
                          <dt>{t('节点', 'Node')}</dt>
                          <dd>{node.name}</dd>
                        </div>
                        <div>
                          <dt>{text('rank')}</dt>
                          <dd>
                            {node.rank ?? 1}
                            {isAdmin && ` ${t('管理员', 'Administrator')}`}
                          </dd>
                        </div>
                        <div>
                          <dt>{t('模型', 'Model')}</dt>
                          <dd>
                            {model
                              ? formatModelConfigLabel(model)
                              : workflowNodeModelLabel(node, models, text)}
                          </dd>
                        </div>
                        <div>
                          <dt>{t('权限', 'Permissions')}</dt>
                          <dd
                            className="workflow-binding-node-tooltip__permission"
                            data-permission={permission}
                          >
                            <PermissionIcon />
                            {permission === 'full'
                              ? t('完全权限', 'Full permissions')
                              : permission === 'custom'
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
                          className={`workflow-node workflow-monitor__node${conversationId ? ' is-bound' : ''}${running ? ' is-running' : ''}`}
                          data-node-id={node.id}
                          data-waiting={
                            waitingApproval ? 'approval' : waitingAnswer ? 'answer' : undefined
                          }
                          aria-label={`${conversationId ? t('双击打开对话', 'Double-click to open conversation') : t('未绑定', 'Unbound')} · ${conversation?.title || node.name}`}
                          onDoubleClick={() => {
                            if (conversationId) {
                              onOpenConversation(conversationId)
                            }
                          }}
                          onKeyDown={(event) => {
                            if (!event.repeat && (event.key === 'Enter' || event.key === ' ')) {
                              event.preventDefault()
                              if (conversationId) onOpenConversation(conversationId)
                            }
                          }}
                        >
                          <svg
                            className="workflow-monitor__node-orbit"
                            aria-hidden="true"
                            focusable="false"
                          >
                            <rect className="workflow-monitor__node-orbit-trail" pathLength="100" />
                            <rect className="workflow-monitor__node-orbit-head" pathLength="100" />
                          </svg>
                          <WorkflowNodeMailbox count={pendingMailByNode.get(node.id) ?? 0} />
                          <AgentAvatar agentId={node.id} className="workflow-node__avatar" />
                          <span className="workflow-node__copy workflow-node__copy--configurable">
                            <strong>{conversation?.title || node.name}</strong>
                            <span>
                              {conversationId
                                ? node.name
                                : t('未绑定对话', 'No conversation bound')}
                            </span>
                          </span>
                          <span className="workflow-monitor__node-indicators">
                            {unread && (
                              <span
                                className="workflow-monitor__node-unread"
                                role="img"
                                aria-label={t('未读消息', 'Unread messages')}
                                title={t('未读消息', 'Unread messages')}
                              />
                            )}
                            <span className="workflow-monitor__node-status" aria-hidden="true" />
                          </span>
                          {snapshot?.pausedConversationIds?.includes(conversationId ?? '') && (
                            <span className="workflow-monitor__stopped">
                              {t('被停止', 'Stopped')}
                            </span>
                          )}
                          {waitingLabel && (
                            <span className="workflow-monitor__node-attention">{waitingLabel}</span>
                          )}
                        </button>
                      </Tooltip>
                    </div>
                  )
                })}
              </div>
            </div>
          </div>
          <div
            className="workflow-canvas-actions"
            onPointerDownCapture={() => dismissActiveTooltip()}
          >
            <div className="workflow-canvas-controls" role="group" aria-label={text('resetView')}>
              <button
                type="button"
                title={text('zoomOut')}
                aria-label={text('zoomOut')}
                disabled={zoom <= 0.15}
                onClick={() => zoomTo(zoom - 0.1)}
              >
                <Minus size={14} />
              </button>
              <button
                type="button"
                className="workflow-canvas-controls__percentage"
                title="100%"
                aria-label="100%"
                onClick={() => zoomTo(1)}
              >
                {Math.round(zoom * 100)}%
              </button>
              <button
                type="button"
                title={text('zoomIn')}
                aria-label={text('zoomIn')}
                disabled={zoom >= 2}
                onClick={() => zoomTo(zoom + 0.1)}
              >
                <Plus size={14} />
              </button>
              <span className="workflow-canvas-controls__separator" />
              <button
                type="button"
                title={text('resetView')}
                aria-label={text('resetView')}
                onClick={resetView}
              >
                <Maximize2 size={14} />
              </button>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
