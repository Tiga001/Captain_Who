import type {
  WorkflowAgentNode,
  WorkflowNodeMessage,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import { ExternalLink, X } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { requestWorkflows } from '../workflowClient'
import { projectWorkflowText } from './projectWorkflowText'
import { workflowNodeQueue } from './workflowNodeQueue'

export function WorkflowNodePanel({
  instanceId,
  node,
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
  const loadedThrough = useRef<number | null>(null)
  const historyRevision = useRef(0)
  const loadingMoreRef = useRef(false)
  const sequence = snapshot?.sequence
  useEffect(() => {
    let disposed = false
    let pending = false
    const refresh = async () => {
      if (pending || loadingMoreRef.current || document.visibilityState === 'hidden') return
      pending = true
      const revision = historyRevision.current
      const oldestLoaded = loadedThrough.current
      try {
        const rows = new Map<number, WorkflowNodeMessage>()
        let beforeSequence: number | undefined
        let nextCursor: number | null = null
        // Refresh the whole displayed range. Settled older mail can leave the
        // compact runtime snapshot, so absence there cannot determine its status.
        while (true) {
          const response = await requestWorkflows({
            operation: 'nodeMessages',
            instanceId,
            nodeId: node.id,
            ...(beforeSequence === undefined ? {} : { beforeSequence })
          })
          if (disposed || revision !== historyRevision.current) return
          const page = response.nodeMessages
          if (!page || page.instanceId !== instanceId || page.nodeId !== node.id)
            throw new Error('Invalid message page')
          page.messages.forEach((row) => rows.set(row.sequence, row))
          nextCursor = page.nextBeforeSequence
          if (
            nextCursor === null ||
            oldestLoaded === null ||
            (page.messages.at(-1)?.sequence ?? Infinity) <= oldestLoaded
          )
            break
          if (beforeSequence !== undefined && nextCursor >= beforeSequence)
            throw new Error('Invalid message pagination cursor')
          beforeSequence = nextCursor
        }
        const messages = [...rows.values()].sort((a, b) => b.sequence - a.sequence)
        loadedThrough.current = messages.at(-1)?.sequence ?? null
        setHistory({ messages, cursor: nextCursor, loaded: true })
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
    if (cursor === null || loadingMoreRef.current) return
    loadingMoreRef.current = true
    historyRevision.current += 1
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
      loadedThrough.current = page.messages.at(-1)?.sequence ?? loadedThrough.current
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
      loadingMoreRef.current = false
      setLoadingMore(false)
      setRetry((value) => value + 1)
    }
  }
  const label = (status: string) =>
    ({
      pending: t('待处理', 'Pending'),
      processing: t('处理中', 'Processing'),
      processed: t('已处理', 'Processed'),
      stopped: t('已停止', 'Stopped'),
      failed: t('失败', 'Failed'),
      recalled: t('已撤回', 'Recalled')
    })[status] ?? t('待处理', 'Pending')
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
              '节点已停止，未处理邮件保留在收件箱中。向对应对话发送消息可恢复。',
              'This node is stopped. Pending mail stays in its inbox. Send a message in the conversation to resume.'
            )}
          </p>
        )}
        <div className="workflow-node-panel__counts">
          <span>
            {t('待处理', 'Pending')}
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
        <section>
          <h3>{t('消息记录', 'Message history')}</h3>
          <p className="workflow-node-panel__muted">
            {t('按收到时间倒序展示工作流来信。', 'Workflow messages, newest first.')}
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
            return (
              <article key={row.message.id} className="workflow-node-panel__message">
                <header>
                  <strong>{row.message.sourceNodeName}</strong>
                  <span>{label(input?.mailStatus ?? row.status)}</span>
                </header>
                <small>
                  {row.message.sourceConversationTitle} ·{' '}
                  {new Date(row.message.createdAt).toLocaleString(language)}
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
