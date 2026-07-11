import { CheckCircle2, SquareTerminal, XCircle } from 'lucide-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { formatToolDetails, getToolCallLabel, type SettledToolStatus } from './toolActivityUtils'

interface GenericToolActivityProps {
  cancelled?: boolean
  call: AgentToolCall
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

export function GenericToolActivity({
  cancelled = false,
  call,
  result,
  settledStatus
}: GenericToolActivityProps) {
  const { t } = useFrontendConfig()
  const hasDetails = Boolean(call.args !== undefined || call.reason || result)
  const status =
    result?.ok === false
      ? 'failed'
      : result
        ? 'completed'
        : (settledStatus ?? (cancelled ? 'cancelled' : 'running'))
  const Icon =
    status === 'failed' ? XCircle : status === 'completed' ? CheckCircle2 : SquareTerminal
  const isPending = status === 'running'

  return (
    <AgentActivityDisclosure
      hasDetails={hasDetails}
      icon={Icon}
      isPending={isPending}
      label={getToolCallLabel(call, result, t, { cancelled, settledStatus })}
    >
      {hasDetails && (
        <div className="agent-activity__details">
          {call.reason && <p>{call.reason}</p>}
          {call.args !== undefined && (
            <>
              <span>{t('agent.detail.args')}</span>
              <pre>{formatToolDetails(call.args)}</pre>
            </>
          )}
          {result?.error && (
            <>
              <span>{t('agent.detail.error')}</span>
              <pre>{result.error}</pre>
            </>
          )}
          {result?.result !== undefined && (
            <>
              <span>{t('agent.detail.result')}</span>
              <pre>{formatToolDetails(result.result)}</pre>
            </>
          )}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
