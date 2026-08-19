import type { AgentToolCall, AgentToolResult, McpBuiltinCapabilityId } from '@mycopilot/protocol'
import { Ban, LoaderCircle, Shield, ShieldCheck, XCircle } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { getBuiltinCapabilityDisplayName } from '../../../mcp/builtinCapabilityPresentation'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

interface ActivateCapabilityToolActivityProps {
  call: AgentToolCall
  cancelled?: boolean
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

type ActivationStatus = 'waiting' | 'running' | 'completed' | 'rejected' | 'failed' | 'cancelled'

function requestedCapability(call: AgentToolCall): McpBuiltinCapabilityId | null {
  if (typeof call.args !== 'object' || call.args === null || Array.isArray(call.args)) return null
  return (call.args as Record<string, unknown>).capability === 'browser_automation'
    ? 'browser_automation'
    : null
}

function resultState(result: AgentToolResult | undefined): string | null {
  if (
    typeof result?.result !== 'object' ||
    result.result === null ||
    Array.isArray(result.result)
  ) {
    return null
  }
  const status = (result.result as Record<string, unknown>).status
  return typeof status === 'string' ? status : null
}

function activationStatus(
  call: AgentToolCall,
  result: AgentToolResult | undefined,
  settledStatus: SettledToolStatus | undefined,
  cancelled: boolean
): ActivationStatus {
  const state = resultState(result)
  if (state === 'active') return 'completed'
  if (['rejected', 'disabled_by_user', 'expired', 'revoked'].includes(state ?? '')) {
    return 'rejected'
  }
  if (result?.ok === false || settledStatus === 'failed') return 'failed'
  if (settledStatus === 'cancelled' || cancelled) return 'cancelled'
  if (result) return 'completed'
  return call.approvalStatus === 'required' ? 'waiting' : 'running'
}

export function ActivateCapabilityToolActivity({
  call,
  cancelled = false,
  result,
  settledStatus
}: ActivateCapabilityToolActivityProps) {
  const { t } = useFrontendConfig()
  const capabilityId = requestedCapability(call)
  const capability = capabilityId
    ? getBuiltinCapabilityDisplayName(capabilityId, t)
    : t('agent.builtinCapability.activation.fallbackName')
  const status = activationStatus(call, result, settledStatus, cancelled)
  const Icon =
    status === 'completed'
      ? ShieldCheck
      : status === 'failed'
        ? XCircle
        : status === 'rejected' || status === 'cancelled'
          ? Ban
          : status === 'waiting'
            ? Shield
            : LoaderCircle

  return (
    <AgentActivityDisclosure
      className="builtin-capability-activation-activity"
      hasDetails={false}
      icon={Icon}
      isPending={status === 'running'}
      label={formatTranslation(t, `agent.builtinCapability.activation.${status}`, { capability })}
    />
  )
}
