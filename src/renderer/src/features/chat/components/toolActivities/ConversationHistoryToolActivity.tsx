import { Brain, CircleStop, CircleX } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

export interface ConversationHistoryActivityItem {
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

interface ConversationHistoryToolActivityProps {
  items: ConversationHistoryActivityItem[]
}

type ConversationHistoryStatus = 'running' | 'completed' | 'failed' | 'cancelled'

function getStatus(items: ConversationHistoryActivityItem[]): ConversationHistoryStatus {
  if (items.some((item) => !item.result && !item.settledStatus)) return 'running'

  const lastItem = items[items.length - 1]
  if (!lastItem) return 'completed'
  if (lastItem.result?.ok === false || lastItem.settledStatus === 'failed') return 'failed'
  if (lastItem.settledStatus === 'cancelled') return 'cancelled'
  return 'completed'
}

export function ConversationHistoryToolActivity({ items }: ConversationHistoryToolActivityProps) {
  const { t } = useFrontendConfig()
  const status = getStatus(items)
  const Icon = status === 'failed' ? CircleX : status === 'cancelled' ? CircleStop : Brain
  const label =
    status === 'running'
      ? t('agent.history.running')
      : status === 'failed'
        ? t('agent.history.failed')
        : status === 'cancelled'
          ? t('agent.history.cancelled')
          : t('agent.history.completed')

  return (
    <AgentActivityDisclosure
      className="agent-activity--status"
      hasDetails={false}
      icon={Icon}
      isPending={status === 'running'}
      label={label}
    />
  )
}
