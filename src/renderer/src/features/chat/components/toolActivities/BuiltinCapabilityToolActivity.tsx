import type { AgentToolIdentity, AgentToolResult } from '@mycopilot/protocol'
import { Ban, CheckCircle2, CircleAlert, LoaderCircle, XCircle } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../../config/frontendTranslations'
import { toSafeMcpDisplayText } from '../../../mcp/mcpSafeDisplay'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import type { SettledToolStatus } from './toolActivityUtils'

type BuiltinCapabilityIdentity = Extract<AgentToolIdentity, { type: 'builtin_capability' }>
type BrowserToolStatus = 'running' | 'completed' | 'failed' | 'cancelled' | 'outcomeUnknown'

interface BuiltinCapabilityToolActivityProps {
  cancelled?: boolean
  displayReason?: string | null
  identity: BuiltinCapabilityIdentity
  result?: AgentToolResult
  settledStatus?: SettledToolStatus
}

const browserToolStatusKeys = {
  browser_navigate: {
    running: 'agent.builtinCapability.browser.navigate.running',
    completed: 'agent.builtinCapability.browser.navigate.completed',
    failed: 'agent.builtinCapability.browser.navigate.failed',
    cancelled: 'agent.builtinCapability.browser.navigate.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.navigate.outcomeUnknown'
  },
  browser_snapshot: {
    running: 'agent.builtinCapability.browser.snapshot.running',
    completed: 'agent.builtinCapability.browser.snapshot.completed',
    failed: 'agent.builtinCapability.browser.snapshot.failed',
    cancelled: 'agent.builtinCapability.browser.snapshot.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.snapshot.outcomeUnknown'
  },
  browser_find: {
    running: 'agent.builtinCapability.browser.find.running',
    completed: 'agent.builtinCapability.browser.find.completed',
    failed: 'agent.builtinCapability.browser.find.failed',
    cancelled: 'agent.builtinCapability.browser.find.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.find.outcomeUnknown'
  },
  browser_click: {
    running: 'agent.builtinCapability.browser.click.running',
    completed: 'agent.builtinCapability.browser.click.completed',
    failed: 'agent.builtinCapability.browser.click.failed',
    cancelled: 'agent.builtinCapability.browser.click.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.click.outcomeUnknown'
  },
  browser_type: {
    running: 'agent.builtinCapability.browser.type.running',
    completed: 'agent.builtinCapability.browser.type.completed',
    failed: 'agent.builtinCapability.browser.type.failed',
    cancelled: 'agent.builtinCapability.browser.type.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.type.outcomeUnknown'
  },
  browser_fill_form: {
    running: 'agent.builtinCapability.browser.fillForm.running',
    completed: 'agent.builtinCapability.browser.fillForm.completed',
    failed: 'agent.builtinCapability.browser.fillForm.failed',
    cancelled: 'agent.builtinCapability.browser.fillForm.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.fillForm.outcomeUnknown'
  },
  browser_press_key: {
    running: 'agent.builtinCapability.browser.pressKey.running',
    completed: 'agent.builtinCapability.browser.pressKey.completed',
    failed: 'agent.builtinCapability.browser.pressKey.failed',
    cancelled: 'agent.builtinCapability.browser.pressKey.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.pressKey.outcomeUnknown'
  },
  browser_tabs: {
    running: 'agent.builtinCapability.browser.tabs.running',
    completed: 'agent.builtinCapability.browser.tabs.completed',
    failed: 'agent.builtinCapability.browser.tabs.failed',
    cancelled: 'agent.builtinCapability.browser.tabs.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.tabs.outcomeUnknown'
  },
  browser_wait_for: {
    running: 'agent.builtinCapability.browser.waitFor.running',
    completed: 'agent.builtinCapability.browser.waitFor.completed',
    failed: 'agent.builtinCapability.browser.waitFor.failed',
    cancelled: 'agent.builtinCapability.browser.waitFor.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.waitFor.outcomeUnknown'
  },
  browser_close: {
    running: 'agent.builtinCapability.browser.close.running',
    completed: 'agent.builtinCapability.browser.close.completed',
    failed: 'agent.builtinCapability.browser.close.failed',
    cancelled: 'agent.builtinCapability.browser.close.cancelled',
    outcomeUnknown: 'agent.builtinCapability.browser.close.outcomeUnknown'
  }
} as const satisfies Record<string, Record<BrowserToolStatus, TranslationKey>>

const fallbackStatusKeys = {
  running: 'agent.builtinCapability.browser.fallback.running',
  completed: 'agent.builtinCapability.browser.fallback.completed',
  failed: 'agent.builtinCapability.browser.fallback.failed',
  cancelled: 'agent.builtinCapability.browser.fallback.cancelled',
  outcomeUnknown: 'agent.builtinCapability.browser.fallback.outcomeUnknown'
} as const satisfies Record<BrowserToolStatus, TranslationKey>

function safeProjectedStatus(result: AgentToolResult | undefined): string | null {
  const value = result?.result
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  if (
    record.schemaVersion !== 1 ||
    record.type !== 'builtin_capability_tool' ||
    record.contentOmitted !== true
  ) {
    return null
  }
  return typeof record.status === 'string' ? record.status : null
}

function getStatus(
  result: AgentToolResult | undefined,
  settledStatus: SettledToolStatus | undefined,
  cancelled: boolean
): BrowserToolStatus {
  const projectedStatus = safeProjectedStatus(result)
  if (projectedStatus === 'outcome_unknown') return 'outcomeUnknown'
  if (projectedStatus === 'cancelled') return 'cancelled'
  if (projectedStatus === 'failed' || result?.ok === false) return 'failed'
  if (projectedStatus === 'completed' || result) return 'completed'
  return settledStatus ?? (cancelled ? 'cancelled' : 'running')
}

function getStatusKey(toolId: string, status: BrowserToolStatus): TranslationKey {
  const toolKeys = Object.hasOwn(browserToolStatusKeys, toolId)
    ? browserToolStatusKeys[toolId as keyof typeof browserToolStatusKeys]
    : fallbackStatusKeys
  return toolKeys[status]
}

/** Product-safe activity for a Host-reviewed managed browser Tool. */
export function BuiltinCapabilityToolActivity({
  cancelled = false,
  displayReason,
  identity,
  result,
  settledStatus
}: BuiltinCapabilityToolActivityProps) {
  const { t } = useFrontendConfig()
  const reason = displayReason ? toSafeMcpDisplayText(displayReason, 512).trim() : ''
  const status = getStatus(result, settledStatus, cancelled)
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

  return (
    <AgentActivityDisclosure
      className="builtin-capability-tool-activity"
      hasDetails={Boolean(reason)}
      icon={Icon}
      isPending={status === 'running'}
      label={t(getStatusKey(identity.toolId, status))}
    >
      {reason ? (
        <div className="agent-activity__details">
          <span>{t('agent.builtinCapability.activity.reason')}</span>
          <p>{reason}</p>
        </div>
      ) : null}
    </AgentActivityDisclosure>
  )
}
