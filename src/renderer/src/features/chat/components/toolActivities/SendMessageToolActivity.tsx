import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { Send } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

export interface SendMessageToolActivityProps {
  call: AgentToolCall
  cancelled?: boolean
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

function getSendMessageStatus({
  cancelled = false,
  result,
  settledStatus
}: SendMessageToolActivityProps) {
  // Only the actual receipt confirms that the recipient's mailbox accepted this message.
  if (result) return result.ok ? 'sent' : 'failed'
  if (cancelled || settledStatus === 'cancelled') return 'cancelled'
  if (settledStatus === 'failed') return 'failed'
  if (settledStatus === 'completed') return 'unknown'
  return 'sending'
}

function textField(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const text = (value as Record<string, unknown>)[key]
  return typeof text === 'string' && text.trim() ? text.trim() : undefined
}

export function SendMessageToolActivity({
  call,
  cancelled = false,
  result,
  settledStatus
}: SendMessageToolActivityProps) {
  const { t } = useFrontendConfig()
  const name =
    (result?.ok ? textField(result.result, 'taskName') : undefined) ??
    textField(call.args, 'target') ??
    t('agent.sendMessage.unknownTarget')
  // A successful receipt means accepted into the recipient's mailbox, not read or processed.
  // Keep that receipt authoritative even if the sender's turn later fails or is cancelled.
  const status = getSendMessageStatus({ call, cancelled, result, settledStatus })

  return (
    <AgentActivityDisclosure
      className="agent-activity--send-message"
      hasDetails={false}
      icon={Send}
      isPending={status === 'sending'}
      label={formatTranslation(t, `agent.sendMessage.${status}`, { name })}
    />
  )
}

export function SendMessageToolActivityGroup({ items }: { items: SendMessageToolActivityProps[] }) {
  const { t } = useFrontendConfig()
  const firstItem = items[0]
  if (!firstItem) return null
  if (items.length === 1) return <SendMessageToolActivity {...firstItem} />

  const statuses = items.map(getSendMessageStatus)
  const status = statuses.includes('sending')
    ? 'sending'
    : statuses.includes('failed')
      ? 'failed'
      : statuses.includes('unknown')
        ? 'unknown'
        : statuses.includes('cancelled')
          ? 'cancelled'
          : 'sent'

  return (
    <AgentActivityDisclosure
      className="agent-activity--send-message-group"
      hasDetails
      icon={Send}
      isPending={status === 'sending'}
      label={formatTranslation(t, `agent.sendMessage.group.${status}`, {
        count: String(items.length)
      })}
    >
      <div className="agent-activity__group-items">
        {items.map((item) => (
          <SendMessageToolActivity key={item.call.id} {...item} />
        ))}
      </div>
    </AgentActivityDisclosure>
  )
}
