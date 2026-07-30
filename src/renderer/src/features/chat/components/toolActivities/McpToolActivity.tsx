import {
  AlertTriangle,
  Ban,
  CheckCircle2,
  CircleHelp,
  Clock3,
  LoaderCircle,
  PlugZap,
  XCircle
} from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../../config/translationFormat'
import { toSafeMcpDisplayText } from '../../../mcp/mcpSafeDisplay'
import type { ChatMcpToolInvocationView } from '../../chatTypes'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import './McpToolActivity.css'

function safePlainText(value: string, maximumLength: number) {
  const normalized = toSafeMcpDisplayText(value, maximumLength).trim()
  if (!normalized) return '\u2014'
  return normalized
}

function getStatusPresentation(
  invocation: ChatMcpToolInvocationView,
  t: ReturnType<typeof useFrontendConfig>['t']
) {
  const tool = `${t('agent.mcp.activity.externalTool')} \u00B7 ${safePlainText(
    invocation.rawToolName,
    1024
  )}`

  switch (invocation.state) {
    case 'pending_approval':
      return {
        Icon: Clock3,
        label: formatTranslation(t, 'agent.tool.waitingApproval', { tool }),
        pending: true
      }
    case 'approved':
    case 'dispatching':
    case 'running':
      return {
        Icon: LoaderCircle,
        label: formatTranslation(t, 'agent.tool.running', { tool }),
        pending: true
      }
    case 'completed':
      return invocation.isError || invocation.outcome === 'tool_error'
        ? { Icon: XCircle, label: t('agent.mcp.activity.toolError'), pending: false }
        : {
            Icon: CheckCircle2,
            label: formatTranslation(t, 'agent.tool.completed', { tool }),
            pending: false
          }
    case 'cancelled':
      return {
        Icon: Ban,
        label: formatTranslation(t, 'agent.tool.cancelled', { tool }),
        pending: false
      }
    case 'rejected':
      return { Icon: Ban, label: t('agent.mcp.activity.rejected'), pending: false }
    case 'expired':
      return { Icon: Clock3, label: t('agent.mcp.activity.expired'), pending: false }
    case 'payload_unavailable':
      return {
        Icon: XCircle,
        label: t('agent.mcp.activity.payloadUnavailable'),
        pending: false
      }
    case 'policy_denied':
      return { Icon: Ban, label: t('agent.mcp.activity.policyDenied'), pending: false }
    case 'outcome_unknown':
      return {
        Icon: CircleHelp,
        label: t('agent.mcp.activity.outcomeUnknown'),
        pending: false
      }
    case 'failed':
      return {
        Icon: XCircle,
        label: formatTranslation(t, 'agent.tool.failed', { tool }),
        pending: false
      }
  }
}

export function McpToolActivity({ invocation }: { invocation?: ChatMcpToolInvocationView }) {
  const { t } = useFrontendConfig()

  if (!invocation) {
    return (
      <div className="agent-activity mcp-tool-activity" role="status">
        <div className="agent-activity__static-summary">
          <span className="agent-activity__icon">
            <PlugZap aria-hidden="true" />
          </span>
          <span className="agent-activity__label">
            {t('agent.mcp.activity.externalTool')} \u00B7{' '}
            {t('agent.mcp.activity.detailUnavailable')}
          </span>
        </div>
      </div>
    )
  }

  const presentation = getStatusPresentation(invocation, t)
  const serverName = safePlainText(invocation.serverDisplayName, 128)
  const toolName = safePlainText(invocation.rawToolName, 1024)
  const errorCode = invocation.errorCode ? safePlainText(invocation.errorCode, 128) : undefined

  return (
    <>
      <span className="mcp-tool-activity__sr-status" role="status">
        {presentation.label}
      </span>
      <AgentActivityDisclosure
        className="mcp-tool-activity"
        defaultOpen={invocation.state === 'outcome_unknown'}
        hasDetails
        icon={presentation.Icon}
        isPending={presentation.pending}
        label={presentation.label}
      >
        <div className="agent-activity__details mcp-tool-activity__details">
          <dl className="mcp-tool-activity__provenance">
            <div>
              <dt>{t('agent.mcp.activity.server')}</dt>
              <dd>{serverName}</dd>
            </div>
            <div>
              <dt>{t('agent.mcp.activity.tool')}</dt>
              <dd>{toolName}</dd>
            </div>
            {invocation.durationMs !== undefined && (
              <div>
                <dt>{t('agent.mcp.activity.duration')}</dt>
                <dd>{Math.max(0, Math.trunc(invocation.durationMs)).toLocaleString()} ms</dd>
              </div>
            )}
            {errorCode && (
              <div>
                <dt>{t('agent.mcp.activity.errorCode')}</dt>
                <dd>{errorCode}</dd>
              </div>
            )}
          </dl>
          {invocation.outputTruncated && (
            <p className="mcp-tool-activity__notice" role="status">
              {t('agent.mcp.activity.outputTruncated')}
            </p>
          )}
          {invocation.state === 'outcome_unknown' && (
            <p className="mcp-tool-activity__warning" role="alert">
              <AlertTriangle aria-hidden="true" />
              <span>{t('agent.mcp.activity.outcomeUnknownWarning')}</span>
            </p>
          )}
        </div>
      </AgentActivityDisclosure>
    </>
  )
}
