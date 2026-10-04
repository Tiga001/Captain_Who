import { ArrowUpRight, Check, Copy } from 'lucide-react'
import { useEffect, useState, type ReactNode } from 'react'
import { Tooltip } from '../../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { useConversationNavigation } from '../../ConversationNavigationContext'
import { copyTextToClipboard } from '../clipboard'
import { WorkflowInboxIcon, WorkflowOutboxIcon } from './WorkflowMailboxIcons'
import { WorkflowQueryFrame } from './WorkflowQueryFrame'
import { WorkflowMailCarousel } from './WorkflowMailCarousel'
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

const mailStates = ['pending', 'processing', 'processed', 'stopped', 'failed', 'recalled'] as const
const historyStates = ['processed', 'stopped', 'failed', 'recalled'] as const
function mailboxCounts(value: unknown): Record<string, number> | null {
  const counts = record(value)
  if (
    !['total', ...mailStates].every(
      (key) =>
        typeof counts[key] === 'number' && Number.isSafeInteger(counts[key]) && counts[key] >= 0
    ) ||
    mailStates.reduce((sum, key) => sum + (counts[key] as number), 0) !== counts.total
  )
    return null
  return counts as Record<string, number>
}

export function WorkflowMailboxToolActivity(props: WorkflowQueryProps) {
  const { language } = useFrontendConfig()
  const chinese = language === 'zh-CN' || language === 'zh-TW'
  const l: Localize = (zh, en) => (chinese ? zh : en)
  const data = record(props.result?.result)
  const outbox = (data.direction ?? record(props.call.args).direction) === 'outbox'
  const returned = records(data.messages)
  const messageId = text(record(props.call.args).messageId).trim()
  const detail = data.view === 'message' || Boolean(messageId)
  const messages = detail
    ? (messageId
        ? returned.filter((message) => text(message.messageId) === messageId)
        : returned
      ).slice(0, 1)
    : outbox
      ? returned
      : returned.filter(
          (message) => message.status === 'pending' || message.status === 'processing'
        )
  const counts = !outbox && !detail ? mailboxCounts(data.counts) : null
  let label = queryLabel(
    props,
    outbox ? l('组织发件箱', 'organization outbox') : l('组织收件箱', 'organization inbox'),
    l
  )
  if (queryStatus(props) === 'completed') {
    if (counts) {
      label += counts.total
        ? l(` · 共 ${counts.total} 封邮件`, ` · ${counts.total} messages total`)
        : l(' · 暂无邮件', ' · No messages')
      for (const state of mailStates)
        if (counts[state]) label += ` · ${stateLabel(state, l)} ${counts[state]}`
    } else {
      const count = detail ? messages.length : returned.length
      label += count
        ? l(` · 本次查到 ${count} 封邮件`, ` · ${count} messages in this result`)
        : l(' · 暂无邮件', ' · No messages')
      // Old receipts contain full historical bodies. They remain summary-only unless the
      // tool explicitly queried one message; never restore the old wall of historical mail.
      if (!outbox && !detail) {
        for (const state of historyStates) {
          const count = returned.filter((message) => message.status === state).length
          if (count) label += ` · ${stateLabel(state, l)} ${count}`
        }
      }
    }
  }
  return (
    <WorkflowQueryFrame
      query={props}
      label={label}
      icon={outbox ? WorkflowOutboxIcon : WorkflowInboxIcon}
      hasMessages={messages.length > 0}
    >
      <WorkflowMailCarousel
        key={props.call.id}
        messages={messages}
        chinese={chinese}
        messageKey={(message, index) => text(message.messageId) || index}
      >
        {(message, _index, navigation) => (
          <MailboxMessage
            message={message}
            outbox={outbox}
            language={language}
            l={l}
            navigation={navigation}
          />
        )}
      </WorkflowMailCarousel>
      {!detail && data.nextCursor != null && (
        <p className="workflow-query__muted">
          {outbox
            ? l(
                '还有更多邮件，本次查询未全部返回。',
                'More messages exist beyond this query result.'
              )
            : l(
                '还有未返回的待处理或处理中的邮件。',
                'More pending or processing messages exist beyond this result.'
              )}
        </p>
      )}
    </WorkflowQueryFrame>
  )
}

export function MailboxMessage({
  message,
  outbox,
  language,
  l,
  navigation
}: {
  message: RecordValue
  outbox: boolean
  language: string
  l: Localize
  navigation?: ReactNode
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
  const copyLabel = copied ? l('已复制', 'Copied') : l('复制邮件', 'Copy message')
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
          {navigation}
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
            {long && !expanded ? `${preview}…` : content || l('（空邮件）', '(Empty message)')}
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
                '本次查询内容较多，未包含这封邮件的正文。',
                'This query omitted the message body due to its size limit.'
              )
            : l('本次查询未提供邮件正文。', 'The message body was not returned.')}
        </p>
      )}
    </section>
  )
}
