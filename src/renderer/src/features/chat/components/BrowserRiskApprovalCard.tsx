import { useEffect, useRef, useState } from 'react'
import type { AgentBrowserRiskKind, AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import { toSafeMcpDisplayText } from '../../mcp/mcpSafeDisplay'
import { ApprovalDialogShell } from './ApprovalDialogShell'

type BrowserRiskAction = Extract<AgentProposedAction, { type: 'browser_risk_approval' }>

interface BrowserRiskApprovalCardProps {
  action: BrowserRiskAction
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

const riskTranslationKeys: Readonly<
  Record<AgentBrowserRiskKind, `agent.browserRisk.risk.${AgentBrowserRiskKind}`>
> = {
  insecure_http: 'agent.browserRisk.risk.insecure_http',
  localhost: 'agent.browserRisk.risk.localhost',
  loopback: 'agent.browserRisk.risk.loopback',
  private_network: 'agent.browserRisk.risk.private_network',
  link_local: 'agent.browserRisk.risk.link_local',
  cloud_metadata: 'agent.browserRisk.risk.cloud_metadata',
  non_standard_port: 'agent.browserRisk.risk.non_standard_port',
  url_userinfo: 'agent.browserRisk.risk.url_userinfo',
  dns_private_resolution: 'agent.browserRisk.risk.dns_private_resolution',
  risk_escalation: 'agent.browserRisk.risk.risk_escalation',
  new_window: 'agent.browserRisk.risk.new_window',
  file_upload: 'agent.browserRisk.risk.file_upload',
  file_download: 'agent.browserRisk.risk.file_download',
  local_service_request: 'agent.browserRisk.risk.local_service_request'
}

function currentUnixSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

function formatUnixSeconds(timestamp: number, language: string): string {
  return new Intl.DateTimeFormat(language, {
    dateStyle: 'short',
    timeStyle: 'medium'
  }).format(new Date(timestamp * 1000))
}

/**
 * Exact, task-scoped browser boundary approval.
 *
 * The component receives only the Host-safe projection and renders untrusted address/reason text
 * through React text nodes. It never sees request headers, cookies, Target/debugger identities or
 * network-resolution fingerprints.
 */
export function BrowserRiskApprovalCard({
  action,
  messageId,
  onApprove,
  onCancel,
  onReject
}: BrowserRiskApprovalCardProps) {
  const { language, t } = useFrontendConfig()
  const submittingRef = useRef(false)
  const { approval } = action
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [rejectMessage, setRejectMessage] = useState('')
  const [expired, setExpired] = useState(() => currentUnixSeconds() >= approval.expiresAt)

  useEffect(() => {
    submittingRef.current = false
    setIsSubmitting(false)
    setRejectMessage('')

    const remainingMs = approval.expiresAt * 1000 - Date.now()
    if (remainingMs <= 0) {
      setExpired(true)
      return
    }

    setExpired(false)
    const timeoutId = window.setTimeout(() => setExpired(true), remainingMs)
    return () => window.clearTimeout(timeoutId)
  }, [approval.actionId, approval.expiresAt, messageId])

  const pending = approval.approvalStatus === 'required'
  const canApprove = pending && !expired && Boolean(onApprove) && !isSubmitting
  const canReject = pending && Boolean(onReject) && !isSubmitting
  const canCancel = pending && Boolean(onCancel) && !isSubmitting

  const submitOnce = (operation: (() => void) | undefined) => {
    if (!operation || submittingRef.current) return
    submittingRef.current = true
    setIsSubmitting(true)
    operation()
  }

  const displayName = toSafeMcpDisplayText(approval.displayName, 256)
  const reason = toSafeMcpDisplayText(approval.reason, 4096)
  const normalizedUrl = toSafeMcpDisplayText(approval.destination.normalizedUrl, 2048)
  const origin = toSafeMcpDisplayText(approval.destination.origin, 512)
  const risks = approval.riskKinds.map((risk) => t(riskTranslationKeys[risk])).join(' · ')
  const createdAt = new Date(approval.createdAt * 1000)
  const expiresAt = new Date(approval.expiresAt * 1000)

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={!canApprove}
      approveLabel={t('agent.approval.dialog.approve')}
      ariaBusy={isSubmitting}
      code={normalizedUrl}
      codeMultiline={false}
      details={
        <dl className="agent-browser-risk-approval__details">
          <div>
            <dt>{t('agent.browserRisk.approval.reason')}</dt>
            <dd>{reason}</dd>
          </div>
          <div>
            <dt>{t('agent.browserRisk.approval.origin')}</dt>
            <dd>{origin}</dd>
          </div>
          <div>
            <dt>{t('agent.browserRisk.approval.risks')}</dt>
            <dd>{risks}</dd>
          </div>
          <div>
            <dt>{t('agent.browserRisk.approval.createdAt')}</dt>
            <dd>
              <time dateTime={createdAt.toISOString()}>
                {formatUnixSeconds(approval.createdAt, language)}
              </time>
            </dd>
          </div>
          <div>
            <dt>{t('agent.browserRisk.approval.expiresAt')}</dt>
            <dd>
              <time dateTime={expiresAt.toISOString()}>
                {formatUnixSeconds(approval.expiresAt, language)}
              </time>
            </dd>
          </div>
        </dl>
      }
      isSubmitting={isSubmitting}
      onApprove={() => submitOnce(() => onApprove?.(messageId, action))}
      onCancel={canCancel ? () => submitOnce(() => onCancel?.(messageId, action)) : undefined}
      onReject={(message) => {
        if (!canReject) return
        const guidance = message.trim()
        submitOnce(() => onReject?.(messageId, action, guidance || undefined))
      }}
      onRejectMessageChange={setRejectMessage}
      policyHint={
        expired
          ? t('agent.browserRisk.approval.expired')
          : t('agent.browserRisk.approval.taskGrantHint')
      }
      policyTone={expired ? 'danger' : 'default'}
      rejectDisabled={!canReject}
      rejectLabel={t('agent.approval.dialog.reject')}
      rejectMessage={rejectMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      request={formatTranslation(t, 'agent.browserRisk.approval.title', {
        capability: displayName
      })}
    />
  )
}
