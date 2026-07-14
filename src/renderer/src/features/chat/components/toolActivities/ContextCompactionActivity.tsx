import { CircleStop, CircleX, Minimize2, RefreshCw } from 'lucide-react'
import type { AgentContextCompactionEventOutcome } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'

interface ContextCompactionActivityProps {
  status: 'running' | AgentContextCompactionEventOutcome
}

export function ContextCompactionActivity({ status }: ContextCompactionActivityProps) {
  const { t } = useFrontendConfig()
  const Icon =
    status === 'failed'
      ? CircleX
      : status === 'cancelled'
        ? CircleStop
        : status === 'skipped'
          ? RefreshCw
          : Minimize2
  const label =
    status === 'running'
      ? t('agent.contextCompaction.running')
      : status === 'failed'
        ? t('agent.contextCompaction.failed')
        : status === 'cancelled'
          ? t('agent.contextCompaction.cancelled')
          : status === 'skipped'
            ? t('agent.contextCompaction.skipped')
            : t('agent.contextCompaction.completed')

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
