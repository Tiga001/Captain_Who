import type {
  WorkflowDefinition,
  WorkflowInstance,
  WorkflowRuntimeInput
} from '@mycopilot/protocol'
import { ArrowLeft, Maximize2, Minus, Plus } from 'lucide-react'
import { useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { dismissActiveTooltip, Tooltip } from '../../../components/overlay/Tooltip'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { AgentAvatar } from '../../agentCollaboration/AgentAvatar'
import { AccountAvatar } from '../../auth/AccountAvatar'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import type { ChatComposerDraft, ChatConversation } from '../../chat/chatTypes'
import type { ConversationAttentionById } from '../../chat/useConversationAttention'
import { getChatPermissionPresentation } from '../../chat/chatPermissionPresentation'
import { formatModelConfigLabel } from '../../modelSelection/modelConfigPresentation'
import { workflowNodeSize } from '../workflowAuthoring'
import { graphBounds, graphFlowLayout } from '../workflowCanvasGeometry'
import { workflowNodeModelLabel, type WorkflowModelDisplay } from '../workflowModelPresentation'
import { workflowText } from '../workflowText'
import { projectWorkflowText } from './projectWorkflowText'
import { useWorkflowMonitor } from './useWorkflowMonitor'
import { useWorkflowExecution } from './useWorkflowExecution'
import '../workflowCanvas.css'
import './workflowBindingCanvas.css'
import './workflowMonitor.css'

interface Props {
  instance: WorkflowInstance
  graph: WorkflowDefinition
  conversations: readonly ChatConversation[]
  conversationDrafts?: Readonly<Record<string, ChatComposerDraft>>
  conversationAttention?: ConversationAttentionById
  models: readonly WorkflowModelDisplay[]
  onBack: () => void
  onOpenConversation: (id: string) => void
}

/** Read-only: the saved workflow remains the source of every node and connection. */
export function WorkflowMonitorPage({
  instance,
  graph,
  conversations,
  conversationDrafts,
  conversationAttention,
  models,
  onBack,
  onOpenConversation
}: Props) {
  const { language } = useFrontendConfig()
  const text = workflowText(language)
  const t = projectWorkflowText(language)
  const profile = useAccountAuth()?.state.profile
  const userName = profile?.displayName || t('当前用户', 'Current user')
  const { snapshot, transmissions, completeUserInput, discardFailedInput } = useWorkflowExecution(
    instance.id
  )
  const [userInput, setUserInput] = useState<WorkflowRuntimeInput | null>(null)
  const [discardInput, setDiscardInput] = useState<WorkflowRuntimeInput | null>(null)
  const [completionError, setCompletionError] = useState<string | null>(null)
  const { runningConversationIds, waitingApprovalConversationIds } = useWorkflowMonitor(
    instance,
    conversations
  )
  const viewportRef = useRef<HTMLDivElement>(null)
  const [size, setSize] = useState({ width: 900, height: 550 })
  const [manualZoom, setManualZoom] = useState<number | null>(null)
  const pan = useRef<{ x: number; y: number; left: number; top: number } | null>(null)
  const markerId = useId().replaceAll(':', '')
  const { geometries } = useMemo(() => graphFlowLayout(graph), [graph])
  const bounds = useMemo(() => graphBounds(graph, geometries), [graph, geometries])
  const bindings = useMemo(
    () => new Map(instance.bindings.map((binding) => [binding.nodeId, binding.conversationId])),
    [instance.bindings]
  )
  const conversationById = useMemo(
    () => new Map(conversations.map((conversation) => [conversation.id, conversation])),
    [conversations]
  )
  const origin = { x: 64 - bounds.left, y: 64 - bounds.top }
  const width = Math.max(400, bounds.right - bounds.left + 128)
  const height = Math.max(260, bounds.bottom - bounds.top + 128)
  const fitZoom = Math.min(1, Math.max(0.15, Math.min(size.width / width, size.height / height)))
  const zoom = manualZoom ?? fitZoom
  const selectedUserNode = graph.nodes.find((node) => node.id === userInput?.nodeId)

  useLayoutEffect(() => {
    const element = viewportRef.current
    if (!element) return
    const observer = new ResizeObserver(() => {
      setSize({ width: element.clientWidth || 900, height: element.clientHeight || 550 })
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  return (
    <section
      className="workflow-monitor"
      aria-label={t('工作流流程图', 'Workflow diagram')}
      style={{ '--workflow-instance-color': instance.color } as CSSProperties}
    >
      <header className="workflow-monitor__header">
        <Tooltip content={t('返回工作流', 'Back to workflows')}>
          <button
            type="button"
            className="workflow-monitor__back"
            aria-label={t('返回工作流', 'Back to workflows')}
            onClick={onBack}
          >
            <ArrowLeft size={18} />
          </button>
        </Tooltip>
        <span className="workflow-monitor__color" aria-hidden="true" />
        <h1>{instance.name}</h1>
      </header>
      <div className="workflow-canvas-shell workflow-binding-canvas-shell workflow-monitor__shell">
        <div
          ref={viewportRef}
          className="workflow-binding-canvas workflow-monitor__canvas"
          aria-label={t('工作流只读画布', 'Read-only workflow canvas')}
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
                      <g key={flow.id} className="workflow-edge" data-flow-id={flow.id}>
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
                        {transmissions
                          .filter((event) => event.flowIds.includes(flow.id))
                          .map((event) => (
                            <circle
                              key={event.sequence}
                              r="4"
                              className="workflow-monitor__transmission"
                              data-transmission-sequence={event.sequence}
                            >
                              <animateMotion path={geometry.path} dur="2s" fill="freeze" />
                            </circle>
                          ))}
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
                  const waiting = snapshot?.inputs.find(
                    (input) => input.nodeId === node.id && input.status === 'waiting_user'
                  )
                  return (
                    <div key={node.id} className="workflow-binding-node-anchor" style={style}>
                      <Tooltip
                        content={
                          <div className="workflow-binding-node-tooltip">
                            <strong>{node.name || userName}</strong>
                            <p>
                              {node.task ||
                                t(
                                  '完成操作后，点击“我已完成”。',
                                  'Complete your task, then choose “I’m done”.'
                                )}
                            </p>
                          </div>
                        }
                        delayMs={1000}
                        delayOnFocus
                        describeTrigger
                        anchorClassName="workflow-binding-node-tooltip-anchor"
                      >
                        <button
                          type="button"
                          className={`workflow-node workflow-monitor__node${waiting ? ' is-bound' : ''}`}
                          data-node-id={node.id}
                          data-waiting={waiting ? 'user' : undefined}
                          disabled={!waiting}
                          onClick={() => {
                            setCompletionError(null)
                            setUserInput(waiting ?? null)
                          }}
                          aria-label={`${node.name || userName}${waiting ? ` · ${t('等待用户操作', 'Waiting for user action')}` : ''}`}
                        >
                          <span className="workflow-node__avatar workflow-user-avatar">
                            <AccountAvatar src={profile?.avatarDataUrl} />
                          </span>
                          <span className="workflow-node__copy">
                            <strong>{userName}</strong>
                            <span>{node.name || t('用户', 'User')}</span>
                          </span>
                          {waiting && (
                            <span className="workflow-monitor__node-attention">
                              {t('等待用户操作', 'Waiting for user action')}
                            </span>
                          )}
                        </button>
                      </Tooltip>
                    </div>
                  )
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
                const failedInput = snapshot?.inputs.find(
                  (input) => input.nodeId === node.id && input.status === 'failed'
                )
                const waitingLabel = waitingApproval
                  ? t('等待批准', 'Waiting for approval')
                  : waitingAnswer
                    ? t('等待交互', 'Waiting for interaction')
                    : failedInput
                      ? t('投递失败', 'Delivery failed')
                      : null
                const statusLabel =
                  waitingLabel ??
                  (running
                    ? t('运行中', 'Running')
                    : conversationId
                      ? t('待命', 'Standby')
                      : t('未绑定', 'Unbound'))
                const draft = conversationId ? conversationDrafts?.[conversationId] : undefined
                const model = models.find(
                  (item) => item.id === (draft?.modelId ?? conversation?.modelId)
                )
                const permission = draft?.permissionMode ?? node.permissionMode
                const PermissionIcon = getChatPermissionPresentation(permission).icon
                const tooltip = (
                  <div className="workflow-binding-node-tooltip">
                    <strong>{conversation?.title || node.name}</strong>
                    <dl>
                      <div>
                        <dt>{t('节点', 'Node')}</dt>
                        <dd>{node.name}</dd>
                      </div>
                      <div>
                        <dt>{t('状态', 'Status')}</dt>
                        <dd>
                          {statusLabel}
                          {unread && ` · ${t('未读消息', 'Unread messages')}`}
                        </dd>
                      </div>
                      {failedInput && (
                        <div className="workflow-binding-node-tooltip__task">
                          <dt>{t('工作流投递', 'Workflow delivery')}</dt>
                          <dd>
                            {t(
                              '有来信未确认送达，后续投递已暂停。',
                              'A delivery could not be confirmed. Later inputs are paused.'
                            )}
                            {failedInput.error && <p>{failedInput.error}</p>}
                          </dd>
                        </div>
                      )}
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
                          waitingApproval
                            ? 'approval'
                            : waitingAnswer
                              ? 'answer'
                              : failedInput
                                ? 'failed'
                                : undefined
                        }
                        aria-label={`${conversationId ? t('打开对话', 'Open conversation') : t('未绑定', 'Unbound')} · ${conversation?.title || node.name}`}
                        disabled={!conversationId}
                        onClick={() => conversationId && onOpenConversation(conversationId)}
                      >
                        <svg
                          className="workflow-monitor__node-orbit"
                          aria-hidden="true"
                          focusable="false"
                        >
                          <rect className="workflow-monitor__node-orbit-trail" pathLength="100" />
                          <rect className="workflow-monitor__node-orbit-head" pathLength="100" />
                        </svg>
                        <AgentAvatar agentId={node.id} className="workflow-node__avatar" />
                        <span className="workflow-node__copy workflow-node__copy--configurable">
                          <strong>{conversation?.title || node.name}</strong>
                          <span>
                            {conversationId ? node.name : t('未绑定对话', 'No conversation bound')}
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
                        {waitingLabel && (waitingApproval || waitingAnswer || !failedInput) && (
                          <span className="workflow-monitor__node-attention">{waitingLabel}</span>
                        )}
                      </button>
                    </Tooltip>
                    {failedInput && (
                      <button
                        type="button"
                        className={`workflow-monitor__node-attention workflow-monitor__delivery-recovery${waitingApproval || waitingAnswer ? ' is-separate' : ''}`}
                        aria-label={t('处理投递失败', 'Resolve failed delivery')}
                        onClick={() => {
                          setCompletionError(null)
                          setDiscardInput(failedInput)
                        }}
                      >
                        {t('投递失败', 'Delivery failed')}
                      </button>
                    )}
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
      {userInput?.instanceId === instance.id && (
        <ConfirmationDialog
          title={t('等待用户操作', 'Waiting for user action')}
          description={
            completionError ?? (selectedUserNode?.kind === 'user' ? selectedUserNode.task : '')
          }
          cancelLabel={t('取消', 'Cancel')}
          confirmLabel={t('我已完成', 'I’m done')}
          confirmVariant="primary"
          onCancel={() => setUserInput(null)}
          onConfirm={async () => {
            try {
              await completeUserInput(userInput.id)
              setUserInput(null)
            } catch {
              setCompletionError(
                t('未能保存完成状态，请重试。', 'Could not save completion. Please retry.')
              )
            }
          }}
        />
      )}
      {discardInput?.instanceId === instance.id && (
        <ConfirmationDialog
          title={t('跳过此来信', 'Skip this input')}
          description={
            completionError ??
            t(
              '这条来信的投递结果无法确认。请先检查对应对话；确认跳过后，继续处理后续来信。',
              'This delivery could not be confirmed. Check the conversation first; skipping this input allows later messages to proceed.'
            )
          }
          cancelLabel={t('取消', 'Cancel')}
          confirmLabel={t('跳过此来信', 'Skip this input')}
          onCancel={() => setDiscardInput(null)}
          onConfirm={async () => {
            try {
              await discardFailedInput(discardInput.id)
              setDiscardInput(null)
            } catch {
              setCompletionError(
                t('未能跳过来信，请重试。', 'Could not skip this input. Please retry.')
              )
            }
          }}
        />
      )}
    </section>
  )
}
