import type { AgentToolIdentity, AgentToolResult } from '@mycopilot/protocol'
import { Ban, CheckCircle2, CircleAlert, LoaderCircle, XCircle } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { toSafeMcpDisplayText } from '../../../mcp/mcpSafeDisplay'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

type BuiltinCapabilityIdentity = Extract<AgentToolIdentity, { type: 'builtin_capability' }>

interface BuiltinCapabilityToolActivityProps {
  cancelled?: boolean
  identity: BuiltinCapabilityIdentity
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

function getCapabilityDisplayName(
  capabilityId: BuiltinCapabilityIdentity['capabilityId'],
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  switch (capabilityId) {
    case 'browser_automation':
      return t('agent.builtinCapability.activity.browserAutomation')
    default: {
      const unreachable: never = capabilityId
      return unreachable
    }
  }
}

function isOutcomeUnknown(result: AgentToolResult | undefined): boolean {
  const value = result?.result
  return (
    typeof value === 'object' &&
    value !== null &&
    !Array.isArray(value) &&
    (value as Record<string, unknown>).type === 'builtin_capability_tool' &&
    (value as Record<string, unknown>).status === 'outcome_unknown'
  )
}

/**
 * Safe activity projection for Host-managed capability tools.
 *
 * Routing is based exclusively on typed Tool identity. This component intentionally never
 * receives the Tool Call, so raw arguments and model-authored reason text cannot accidentally
 * enter its DOM. It also never renders Tool result or error bodies; those may contain private
 * browser state even after the model-facing projection has been bounded.
 */
export function BuiltinCapabilityToolActivity({
  cancelled = false,
  identity,
  result,
  settledStatus
}: BuiltinCapabilityToolActivityProps) {
  const { t } = useFrontendConfig()
  const capability = getCapabilityDisplayName(identity.capabilityId, t)
  const tool = toSafeMcpDisplayText(identity.modelName, 128).trim() || 'browser_tool'
  const status = isOutcomeUnknown(result)
    ? 'outcomeUnknown'
    : result?.ok === false
      ? 'failed'
      : result
        ? 'completed'
        : (settledStatus ?? (cancelled ? 'cancelled' : 'running'))
  const Icon =
    status === 'outcomeUnknown'
      ? CircleAlert
      : status === 'failed'
        ? XCircle
        : status === 'completed'
          ? CheckCircle2
          : status === 'cancelled'
            ? Ban
            : LoaderCircle
  const label = formatTranslation(t, `agent.builtinCapability.activity.${status}`, {
    capability,
    tool
  })

  return (
    <AgentActivityDisclosure
      className="builtin-capability-tool-activity"
      hasDetails={false}
      icon={Icon}
      isPending={status === 'running'}
      label={label}
    />
  )
}
