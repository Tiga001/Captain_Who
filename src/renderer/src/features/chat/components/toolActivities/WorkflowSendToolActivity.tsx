import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { Check, Copy, ArrowUpRight, Network } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Tooltip } from '../../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { useConversationNavigation } from '../../ConversationNavigationContext'
import { copyTextToClipboard } from '../clipboard'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'
import './WorkflowSendToolActivity.css'

interface WorkflowSendToolActivityProps {
  call: AgentToolCall
  cancelled?: boolean
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

type ObjectValue = Record<string, unknown>
function object(value: unknown): ObjectValue | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as ObjectValue)
    : undefined
}
function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}
function outputs(value: unknown): ObjectValue[] {
  if (!Array.isArray(value)) return []
  return value
    .slice(0, 512)
    .map(object)
    .filter((item): item is ObjectValue => Boolean(item))
}

function WorkflowMessageBox({
  name,
  message,
  conversationId,
  chinese
}: {
  name: string
  message: string
  conversationId?: string
  chinese: boolean
}) {
  const openConversation = useConversationNavigation()
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 1300)
    return () => window.clearTimeout(timer)
  }, [copied])
  const copyLabel = copied ? (chinese ? '已复制' : 'Copied') : chinese ? '复制消息' : 'Copy message'
  const jumpLabel = chinese ? '打开对话' : 'Open conversation'
  return (
    <section className="workflow-send-message" aria-label={name}>
      <div className="workflow-send-message__header">
        <span className="workflow-send-message__target">{name}</span>
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
          <Tooltip content={copyLabel}>
            <button
              type="button"
              aria-label={`${copyLabel} · ${name}`}
              disabled={!message}
              onClick={() => {
                void copyTextToClipboard(message)
                  .then(() => setCopied(true))
                  .catch(() => setCopied(false))
              }}
            >
              {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
            </button>
          </Tooltip>
        </div>
      </div>
      <div className="workflow-send-message__body">{message}</div>
    </section>
  )
}

export function WorkflowSendToolActivity({
  call,
  cancelled = false,
  result,
  settledStatus
}: WorkflowSendToolActivityProps) {
  const { language } = useFrontendConfig()
  const chinese = language === 'zh-CN' || language === 'zh-TW'
  const args = object(call.args)
  // Receipts freeze display names at send time. Never resolve old messages against mutable graphs.
  const metadata = (result?.ok ? object(result.result) : undefined) ?? object(args?._workflowSend)
  const workflowName = text(metadata?.workflowName)
  const destinations = outputs(metadata?.outputs)
  const messages = outputs(args?.outputs).map((output, index) => {
    const target = destinations.find((item) => item.flowId === output.flowId)
    return {
      name:
        text(target?.targetNodeName) ??
        (chinese ? `接收节点 ${index + 1}` : `Recipient ${index + 1}`),
      conversationId: text(target?.targetConversationId),
      message: typeof output.message === 'string' ? output.message : ''
    }
  })
  const names = [...new Set(destinations.flatMap((item) => text(item.targetNodeName) ?? []))]
  const target = workflowName
    ? chinese
      ? `工作流【${workflowName}】${names.length ? `的节点【${names.join('、')}】` : ''}`
      : `workflow “${workflowName}”${names.length ? `, nodes “${names.join('”, “')}”` : ''}`
    : chinese
      ? '工作流节点'
      : 'workflow nodes'
  // A later cancellation does not erase a committed receipt; a completed turn without a receipt
  // does not prove a message was sent.
  const status = result
    ? result.ok
      ? 'sent'
      : 'failed'
    : cancelled || settledStatus === 'cancelled'
      ? 'cancelled'
      : settledStatus === 'failed'
        ? 'failed'
        : settledStatus === 'completed'
          ? 'unknown'
          : 'sending'
  const label = chinese
    ? {
        sending: `正在向${target}发送消息`,
        sent: `已向${target}发送了消息`,
        failed: `向${target}发送消息失败`,
        cancelled: `已取消向${target}发送消息`,
        unknown: `向${target}发送消息的结果待确认`
      }[status]
    : {
        sending: `Sending a message to ${target}`,
        sent: `Sent a message to ${target}`,
        failed: `Could not send a message to ${target}`,
        cancelled: `Cancelled sending a message to ${target}`,
        unknown: `Message delivery to ${target} is unconfirmed`
      }[status]
  return (
    <AgentActivityDisclosure
      className="agent-activity--workflow-send"
      hasDetails={messages.length > 0 || Boolean(result?.error)}
      icon={Network}
      isPending={status === 'sending'}
      label={label}
    >
      <div className="workflow-send-messages">
        {messages.map((message, index) => (
          <WorkflowMessageBox key={index} {...message} chinese={chinese} />
        ))}
        {result?.error && <p className="workflow-send-messages__error">{result.error}</p>}
      </div>
    </AgentActivityDisclosure>
  )
}
