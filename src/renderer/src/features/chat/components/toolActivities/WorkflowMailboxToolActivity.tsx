import { ArrowUpRight, Check, Copy } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Tooltip } from '../../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { useConversationNavigation } from '../../ConversationNavigationContext'
import { copyTextToClipboard } from '../clipboard'
import { WorkflowInboxIcon, WorkflowOutboxIcon } from './WorkflowMailboxIcons'
import { WorkflowQueryFrame } from './WorkflowQueryFrame'
import {
  queryLabel,
  queryStatus,
  record,
  records,
  stateLabel,
  stateTone,
  text,
  timeLabel,
  type Localize,
  type RecordValue,
  type WorkflowQueryProps
} from './workflowQueryPresentation'
import './WorkflowSendToolActivity.css'

export function WorkflowMailboxToolActivity(props: WorkflowQueryProps) {
  const { language } = useFrontendConfig()
  const chinese = language === 'zh-CN' || language === 'zh-TW'
  const l: Localize = (zh, en) => (chinese ? zh : en)
  const data = record(props.result?.result)
  const outbox = (data.direction ?? record(props.call.args).direction) === 'outbox'
  const messages = records(data.messages)
  let label = queryLabel(
    props,
    outbox ? l('工作流发件箱', 'workflow outbox') : l('工作流收件箱', 'workflow inbox'),
    l
  )
  if (queryStatus(props) === 'completed')
    label += messages.length
      ? l(` · 本次查到 ${messages.length} 条消息`, ` · ${messages.length} messages in this result`)
      : l(' · 暂无消息', ' · No messages')
  const groups = new Map<string, RecordValue[]>()
  for (const message of messages) {
    const status = text(message.status)
    const group =
      status === 'pending' ? 'pending' : status === 'processing' ? 'processing' : 'history'
    groups.set(group, [...(groups.get(group) ?? []), message])
  }
  return (
    <WorkflowQueryFrame
      query={props}
      label={label}
      icon={outbox ? WorkflowOutboxIcon : WorkflowInboxIcon}
      l={l}
      hasMessages={messages.length > 0}
    >
      {['pending', 'processing', 'history']
        .filter((key) => groups.has(key))
        .map((key) => (
          <div key={key} className="workflow-query__group">
            <small>
              {key === 'pending'
                ? l('待处理', 'Pending')
                : key === 'processing'
                  ? l('处理中', 'Processing')
                  : l('历史消息', 'History')}
            </small>
            {groups.get(key)!.map((message, index) => (
              <MailboxMessage
                key={text(message.messageId) || index}
                message={message}
                outbox={outbox}
                language={language}
                l={l}
              />
            ))}
          </div>
        ))}
      {data.nextCursor != null && (
        <p className="workflow-query__muted">
          {l('还有更多记录，本次查询未全部返回。', 'More records exist beyond this query result.')}
        </p>
      )}
    </WorkflowQueryFrame>
  )
}

export function MailboxMessage({
  message,
  outbox,
  language,
  l
}: {
  message: RecordValue
  outbox: boolean
  language: string
  l: Localize
}) {
  const openConversation = useConversationNavigation()
  const [expanded, setExpanded] = useState(false)
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 1300)
    return () => window.clearTimeout(timer)
  }, [copied])
  const name =
    text(outbox ? message.targetNodeName : message.sourceNodeName) ||
    l(outbox ? '接收节点' : '来源节点', outbox ? 'Recipient' : 'Sender')
  const conversationId = text(outbox ? message.targetConversationId : message.sourceConversationId)
  const content = message.bodyAvailable === true ? text(message.content) : ''
  const canCopy = message.bodyAvailable === true && typeof message.content === 'string'
  const long = content.length > 320 || content.split('\n').length > 6
  const preview = content.slice(0, 320).split('\n').slice(0, 6).join('\n')
  const copyLabel = copied ? l('已复制', 'Copied') : l('复制消息', 'Copy message')
  const jumpLabel = l('打开对话', 'Open conversation')
  const time = timeLabel(message.createdAt, language)
  return (
    <section
      className="workflow-send-message"
      aria-label={l(
        outbox ? `发给 ${name}` : `来自 ${name}`,
        outbox ? `To ${name}` : `From ${name}`
      )}
    >
      <div className="workflow-send-message__header">
        <span className="workflow-send-message__target">
          {l(outbox ? '发给 ' : '来自 ', outbox ? 'To ' : 'From ')}
          {name}
        </span>
        <div className="workflow-send-message__actions">
          {conversationId && openConversation && (
            <Tooltip content={jumpLabel}>
              <button
                type="button"
                aria-label={`${jumpLabel} · ${name}`}
                onClick={() => openConversation(conversationId)}
              >
                <ArrowUpRight aria-hidden="true" />
              </button>
            </Tooltip>
          )}
          {canCopy && (
            <Tooltip content={copyLabel}>
              <button
                type="button"
                aria-label={`${copyLabel} · ${name}`}
                onClick={() => {
                  void copyTextToClipboard(content)
                    .then(() => setCopied(true))
                    .catch(() => setCopied(false))
                }}
              >
                {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
              </button>
            </Tooltip>
          )}
        </div>
      </div>
      <div className="workflow-query__message-meta">
        <span className="workflow-query__status" data-tone={stateTone(message.status)}>
          {stateLabel(message.status, l)}
        </span>
        {time && <time>{time}</time>}
      </div>
      {canCopy ? (
        <>
          <div className="workflow-send-message__body">
            {long && !expanded ? `${preview}…` : content || l('（空消息）', '(Empty message)')}
          </div>
          {long && (
            <button
              className="workflow-query__expand"
              type="button"
              aria-expanded={expanded}
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? l('收起正文', 'Collapse message') : l('展开全文', 'Read full message')}
            </button>
          )}
        </>
      ) : (
        <p className="workflow-query__withheld">
          {message.withholdingReason === 'response_body_budget'
            ? l(
                '本次查询内容较多，未包含这条消息的正文。',
                'This query omitted the message body due to its size limit.'
              )
            : l('本次查询未提供消息正文。', 'The message body was not returned.')}
        </p>
      )}
    </section>
  )
}
