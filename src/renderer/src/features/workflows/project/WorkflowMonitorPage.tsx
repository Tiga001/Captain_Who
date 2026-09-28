import type {
  WorkflowDefinition,
  WorkflowInstance,
  WorkflowRuntimeInput
} from '@mycopilot/protocol'
import { ArrowLeft, Maximize2, Minus, Plus } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState, type CSSProperties } from 'react'
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
import { useWorkflowMonitorViewport } from './useWorkflowMonitorViewport'
import { WorkflowGateTooltip } from './WorkflowGateTooltip'
import { WorkflowNodePanel } from './WorkflowNodePanel'
import { WorkflowInputGateQueue } from './WorkflowInputGateQueue'
import { inputGateRecipient, workflowNodeQueue } from './workflowNodeQueue'
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
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null)
  const selectionTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const cancelNodeSelection = () => {
    if (selectionTimer.current !== null) {
      clearTimeout(selectionTimer.current)
      selectionTimer.current = null
    }
  }
  useEffect(
    () => () => {
      if (selectionTimer.current !== null) clearTimeout(selectionTimer.current)
    },
    [instance.id]
  )
  const selectNode = (nodeId: string) => {
    cancelNodeSelection()
    // Keep the target in place until the browser can deliver a second click.
    // Opening the panel immediately would cover it or resize the canvas.
    selectionTimer.current = setTimeout(() => {
      selectionTimer.current = null
      setSelectedNodeId(nodeId)
    }, 300)
  }
  const [userInput, setUserInput] = useState<WorkflowRuntimeInput | null>(null)
  const [discardInput, setDiscardInput] = useState<WorkflowRuntimeInput | null>(null)
  const [completionError, setCompletionError] = useState<string | null>(null)
  const { runningConversationIds, waitingApprovalConversationIds } = useWorkflowMonitor(
    instance,
    conversations
  )
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
  const { viewportRef, size, zoom, left, top, dragging, zoomTo, resetView, panHandlers } =
    useWorkflowMonitorViewport(width, height)
  const selectedUserNode = graph.nodes.find((node) => node.id === userInput?.nodeId)

  const selectedNode = graph.nodes.find(
    (node) => node.id === selectedNodeId && node.kind === 'agent'
  )
  const selectedConversationId = selectedNode ? bindings.get(selectedNode.id) : undefined
  const nodeStatus = (nodeId: string, conversationId?: string) => {
    if (!conversationId) return t('未绑定', 'Unbound')
    const paused = snapshot?.pausedConversationIds?.includes(conversationId)
    if (paused) return t('被停止', 'Stopped')
    const attention = conversationAttention?.[conversationId]
    const latestRun = conversationById
      .get(conversationId)
      ?.messages.findLast((message) => message.role === 'assistant')?.agentRun
    if (
      waitingApprovalConversationIds?.has(conversationId) ||
      (attention?.waitingApproval ?? latestRun?.status === 'waiting_for_approval')
    )
      return t('等待批准', 'Waiting for approval')
    if (attention?.waitingAnswer ?? latestRun?.status === 'waiting_for_user_input')
      return t('等待交互', 'Waiting for interaction')
    if (runningConversationIds.has(conversationId)) return t('活跃中', 'Active')
    if (!snapshot) return t('正在读取状态', 'Loading state')
    const queue = workflowNodeQueue(snapshot, nodeId)
    if (queue.inputs.some((input) => input.status === 'failed'))
      return t('投递受阻', 'Delivery blocked')
    if (!instance.enabled) return t('工作流已关闭', 'Workflow disabled')
    if (queue.waiting.length) return t('等待处理', 'Waiting to process')
    if (queue.collecting.length) return t('等待凑齐批次', 'Collecting batch')
    return t('待命', 'Standby')
  }

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
      <div className="workflow-monitor__body">
        <div className="workflow-canvas-shell workflow-binding-canvas-shell workflow-monitor__shell">
          <div
            ref={viewportRef}
            className={`workflow-binding-canvas workflow-monitor__canvas${dragging ? ' is-panning' : ''}`}
            aria-label={t('工作流只读画布', 'Read-only workflow canvas')}
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
                    <AccountAvatar
                      src={profile?.avatarDataUrl}
                      localAvatarSeed={profile?.localAccount?.avatarSeed}
                    />
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
                    const recipient =
                      node.kind === 'inputGate' ? inputGateRecipient(graph, node.id) : null
                    const queued = recipient
                      ? workflowNodeQueue(snapshot, recipient).queuedMessages.length
                      : 0
                    return (
                      <div key={node.id} className="workflow-binding-node-anchor" style={style}>
                        <Tooltip
                          content={
                            <>
                              <WorkflowGateTooltip node={node} graph={graph} text={text} />
                              {recipient && (
                                <p>
                                  {t('积压消息', 'Backlog')}: {snapshot ? queued : '—'}
                                  {queued > 9 ? ` · ${t('消息积压较多', 'High backlog')}` : ''}
                                </p>
                              )}
                            </>
                          }
                          delayMs={1000}
                          delayOnFocus
                          describeTrigger
                          anchorClassName="workflow-binding-node-tooltip-anchor"
                        >
                          <div
                            className={`workflow-node workflow-node--gate workflow-node--${node.kind} workflow-monitor__gate`}
                            data-node-id={node.id}
                            tabIndex={0}
                            role="group"
                            aria-label={node.name || text(node.kind)}
                          >
                            <svg
                              className="workflow-gate-shape"
                              viewBox="0 0 64 56"
                              aria-hidden="true"
                            >
                              <path
                                d={
                                  node.kind === 'inputGate'
                                    ? 'M 2 2 L 62 28 L 2 54 Z'
                                    : 'M 62 2 L 2 28 L 62 54 Z'
                                }
                              />
                            </svg>
                            {recipient && (
                              <WorkflowInputGateQueue count={queued} color={instance.color} />
                            )}
                          </div>
                        </Tooltip>
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
                              <AccountAvatar
                                src={profile?.avatarDataUrl}
                                localAvatarSeed={profile?.localAccount?.avatarSeed}
                              />
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
                    (snapshot?.pausedConversationIds?.includes(conversationId ?? '')
                      ? t('被停止', 'Stopped')
                      : null) ??
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
                          className={`workflow-node workflow-monitor__node${conversationId ? ' is-bound' : ''}${running ? ' is-running' : ''}${selectedNodeId === node.id ? ' is-selected' : ''}`}
                          aria-pressed={selectedNodeId === node.id}
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
                          aria-label={`${conversationId ? t('双击打开对话', 'Double-click to open conversation') : t('未绑定', 'Unbound')} · ${conversation?.title || node.name}`}
                          onClick={(event) => {
                            if (event.detail < 2) selectNode(node.id)
                          }}
                          onDoubleClick={() => {
                            if (conversationId) {
                              cancelNodeSelection()
                              onOpenConversation(conversationId)
                            }
                          }}
                          onKeyDown={(event) => {
                            if (!event.repeat && (event.key === 'Enter' || event.key === ' ')) {
                              event.preventDefault()
                              cancelNodeSelection()
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
        {selectedNode?.kind === 'agent' && (
          <WorkflowNodePanel
            key={`${instance.id}:${selectedNode.id}`}
            instanceId={instance.id}
            node={selectedNode}
            graph={graph}
            snapshot={snapshot}
            title={conversationById.get(selectedConversationId ?? '')?.title || selectedNode.name}
            status={nodeStatus(selectedNode.id, selectedConversationId)}
            conversationId={selectedConversationId}
            currentTask={
              conversationById
                .get(selectedConversationId ?? '')
                ?.messages.findLast((message) => message.role === 'user')?.content
            }
            onClose={() => setSelectedNodeId(null)}
            onOpenConversation={onOpenConversation}
          />
        )}
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
