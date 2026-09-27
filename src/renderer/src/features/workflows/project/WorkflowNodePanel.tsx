import type {
  WorkflowAgentNode,
  WorkflowDefinition,
  WorkflowNodeMessage,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import { ExternalLink, X } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { requestWorkflows } from '../workflowClient'
import { projectWorkflowText } from './projectWorkflowText'
import { workflowNodeQueue } from './workflowNodeQueue'

export function WorkflowNodePanel({
  instanceId,
  node,
  graph,
  snapshot,
  title,
  status,
  conversationId,
  currentTask,
  onClose,
  onOpenConversation
}: {
  instanceId: string
  node: WorkflowAgentNode
  graph: WorkflowDefinition
  snapshot: WorkflowRuntimeSnapshot | null
  title: string
  status: string
  conversationId?: string
  currentTask?: string
  onClose: () => void
  onOpenConversation: (id: string) => void
}) {
  const { language } = useFrontendConfig()
  const t = projectWorkflowText(language)
  const queue = workflowNodeQueue(snapshot, node.id)
  const [{ messages, cursor, loaded }, setHistory] = useState<{
    messages: WorkflowNodeMessage[]
    cursor: number | null
    loaded: boolean
  }>({ messages: [], cursor: null, loaded: false })
  const [error, setError] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [retry, setRetry] = useState(0)
  const sequence = snapshot?.sequence
  useEffect(() => {
    let disposed = false
    let pending = false
    const refresh = async () => {
      if (pending || document.visibilityState === 'hidden') return
      pending = true
      try {
        const response = await requestWorkflows({
          operation: 'nodeMessages',
          instanceId,
          nodeId: node.id
        })
        const page = response.nodeMessages
        if (!page || page.instanceId !== instanceId || page.nodeId !== node.id)
          throw new Error('Invalid message page')
        if (disposed) return
        setHistory((previous) => {
          // If more than a page arrived, reset to the new head so pagination cannot skip the gap.
          const overlap = previous.messages.some((old) =>
            page.messages.some((row) => row.sequence === old.sequence)
          )
          const rows = new Map((overlap ? previous.messages : []).map((row) => [row.sequence, row]))
          page.messages.forEach((row) => rows.set(row.sequence, row))
          return {
            messages: [...rows.values()].sort((a, b) => b.sequence - a.sequence),
            cursor: overlap ? previous.cursor : page.nextBeforeSequence,
            loaded: true
          }
        })
        setError(false)
      } catch {
        if (!disposed) setError(true)
      } finally {
        pending = false
      }
    }
    void refresh()
    const timer = setInterval(() => void refresh(), 5000)
    return () => {
      disposed = true
      clearInterval(timer)
    }
  }, [instanceId, node.id, sequence, retry])
  const more = async () => {
    if (cursor === null || loadingMore) return
    setLoadingMore(true)
    try {
      const response = await requestWorkflows({
        operation: 'nodeMessages',
        instanceId,
        nodeId: node.id,
        beforeSequence: cursor
      })
      const page = response.nodeMessages
      if (!page || page.instanceId !== instanceId || page.nodeId !== node.id)
        throw new Error('Invalid message page')
      setHistory((previous) => {
        if (previous.cursor !== cursor) return previous
        const rows = new Map(previous.messages.map((row) => [row.sequence, row]))
        page.messages.forEach((row) => rows.set(row.sequence, row))
        return {
          messages: [...rows.values()].sort((a, b) => b.sequence - a.sequence),
          cursor: page.nextBeforeSequence,
          loaded: true
        }
      })
      setError(false)
    } catch {
      setError(true)
    } finally {
      setLoadingMore(false)
    }
  }
  const gate = graph.nodes.find(
    (candidate) =>
      candidate.kind === 'inputGate' &&
      graph.flows.some(
        (flow) =>
          flow.source.kind === 'node' &&
          flow.source.nodeId === candidate.id &&
          flow.target.kind === 'node' &&
          flow.target.nodeId === node.id
      )
  )
  const incoming = gate
    ? graph.flows.filter((flow) => flow.target.kind === 'node' && flow.target.nodeId === gate.id)
    : []
  const label = (inputStatus: string, runStatus: string | null) => {
    if (inputStatus === 'applied' && runStatus) {
      if (runStatus === 'completed') return t('已完成', 'Completed')
      if (runStatus === 'cancelled') return t('被停止', 'Stopped')
      if (runStatus === 'failed') return t('运行失败', 'Run failed')
      if (runStatus === 'in_progress') return t('处理中', 'Processing')
      return t('运行已结束', 'Run ended')
    }
    return (
      {
        collecting: t('等待凑齐批次', 'Collecting batch'),
        pending: t('排队中', 'Queued'),
        paused: t('暂停等待', 'Paused'),
        claimed: t('准备投递', 'Delivering'),
        applied: t('已送达', 'Delivered'),
        failed: t('投递失败', 'Delivery failed'),
        invalidated: t('已失效', 'Invalidated'),
        completed: t('已完成', 'Completed'),
        waiting_user: t('等待用户操作', 'Waiting for user')
      }[inputStatus] ?? inputStatus
    )
  }
  return (
    <aside className="workflow-node-panel" aria-label={t('节点看板', 'Node dashboard')}>
      <header>
        <div>
          <h2>{title}</h2>
          <span>
            {node.name} · {status}
          </span>
        </div>
        <button
          type="button"
          onClick={onClose}
          aria-label={t('收起节点看板', 'Close node dashboard')}
        >
          <X size={18} />
        </button>
      </header>
      <div className="workflow-node-panel__body">
        {snapshot?.pausedConversationIds?.includes(conversationId ?? '') && (
          <p className="workflow-node-panel__notice">
            {t(
              '你已停止此节点，后续消息会保留在队列中。向对应对话手动发送消息可恢复工作流接收；排队模式会等待该轮结束。',
              'This node was stopped. Messages stay queued. Send a message in the conversation to resume; queued delivery waits for that turn to finish.'
            )}
          </p>
        )}
        <div className="workflow-node-panel__counts">
          <span>
            {t('积压消息', 'Backlog')}
            <strong>{snapshot ? queue.queuedMessages.length : '—'}</strong>
          </span>
          <span>
            {t('待处理批次', 'Queued batches')}
            <strong>{snapshot ? queue.waiting.length : '—'}</strong>
          </span>
          <span>
            {t('处理中', 'Processing')}
            <strong>{snapshot ? queue.processing.length : '—'}</strong>
          </span>
        </div>
        <section>
          <h3>{t('当前处理', 'Current work')}</h3>
          {queue.processing.length ? (
            queue.processing.map((input) => (
              <p key={input.id}>
                {input.messages.map((message) => message.sourceNodeName).join('、')} ·{' '}
                {input.messages.length} {t('条消息', 'messages')}
              </p>
            ))
          ) : (
            <p className="workflow-node-panel__muted">
              {currentTask && status === t('活跃中', 'Active')
                ? currentTask
                : status === t('活跃中', 'Active')
                  ? t(
                      '正在处理对话任务，暂无关联的工作流输入。',
                      'Working on a conversation task with no linked workflow input.'
                    )
                  : t('暂无正在处理的工作流输入', 'No workflow input is being processed')}
            </p>
          )}
        </section>
        {gate?.kind === 'inputGate' && gate.processingMode === 'batch' && (
          <section>
            <h3>{t('下一批到达情况', 'Next batch arrivals')}</h3>
            {incoming.map((flow) => {
              const source = flow.source
              const name =
                source.kind === 'node'
                  ? graph.nodes.find((item) => item.id === source.nodeId)?.name
                  : t('用户', 'User')
              const count = queue.collecting.filter(
                (message) => message.flowId === flow.id || message.pathFlowIds.includes(flow.id)
              ).length
              return (
                <p key={flow.id} className="workflow-node-panel__arrival">
                  <span>
                    {flow.name} · {name}
                  </span>
                  <strong>
                    {count ? `${count} ${t('条已到达', 'arrived')}` : t('等待到达', 'Waiting')}
                  </strong>
                </p>
              )
            })}
          </section>
        )}
        <section>
          <h3>{t('消息记录', 'Message history')}</h3>
          <p className="workflow-node-panel__muted">
            {t(
              '包含待收集、排队、处理中和已结束的来信，按收到时间倒序。相同批次编号的消息一起处理。',
              'Incoming messages, newest first. Matching batch numbers are processed together.'
            )}
          </p>
          {error && (
            <p role="alert">
              {t('消息读取失败', 'Could not load messages')}{' '}
              <button type="button" onClick={() => setRetry((value) => value + 1)}>
                {t('重试', 'Retry')}
              </button>
            </p>
          )}
          {!loaded && !error && <p>{t('正在读取消息…', 'Loading messages…')}</p>}
          {loaded && messages.length === 0 && (
            <p>{t('暂无工作流来信', 'No workflow messages yet')}</p>
          )}
          {messages.map((row) => {
            const input = snapshot?.inputs.find(
              (item) =>
                item.id === row.inputId ||
                item.messages.some((message) => message.id === row.message.id)
            )
            const batchId = input?.id ?? row.inputId
            const runStatus = (input && queue.runs.get(input.id)) ?? row.runStatus
            return (
              <article key={row.message.id} className="workflow-node-panel__message">
                <header>
                  <strong>{row.message.sourceNodeName}</strong>
                  <span>{label(input?.status ?? row.status, runStatus)}</span>
                </header>
                <small>
                  {row.message.sourceConversationTitle} ·{' '}
                  {new Date(row.message.createdAt).toLocaleString(language)}
                  {batchId ? ` · ${t('批次', 'Batch')} ${batchId.slice(0, 8)}` : ''}
                </small>
                <p>{row.message.content}</p>
                {(input?.error ?? row.error) && (
                  <p className="workflow-node-panel__notice">{input?.error ?? row.error}</p>
                )}
              </article>
            )
          })}
          {cursor !== null && (
            <button
              type="button"
              className="workflow-node-panel__more"
              disabled={loadingMore}
              onClick={() => void more()}
            >
              {t('加载更早消息', 'Load older messages')}
            </button>
          )}
        </section>
        {conversationId && (
          <button
            type="button"
            className="workflow-node-panel__open"
            onClick={() => onOpenConversation(conversationId)}
          >
            <ExternalLink size={15} />
            {t('进入对话', 'Open conversation')}
          </button>
        )}
      </div>
    </aside>
  )
}
