import { useEffect, useRef, useState } from 'react'
import type { AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import { getBuiltinCapabilityDisplayName } from '../../mcp/builtinCapabilityPresentation'
import { toSafeMcpDisplayText } from '../../mcp/mcpSafeDisplay'
import { ApprovalDialogShell } from './ApprovalDialogShell'

type BuiltinCapabilityActivationAction = Extract<
  AgentProposedAction,
  { type: 'builtin_capability_activation' }
>

interface BuiltinCapabilityActivationApprovalCardProps {
  action: BuiltinCapabilityActivationAction
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

function currentUnixSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

/**
 * Approval for a Host-owned built-in capability grant.
 *
 * This intentionally renders only the frozen presentation fields. Managed Server identity,
 * transport configuration, manifest contents and reviewed Tool lists never enter this component.
 */
export function BuiltinCapabilityActivationApprovalCard({
  action,
  messageId,
  onApprove,
  onCancel,
  onReject
}: BuiltinCapabilityActivationApprovalCardProps) {
  const { t } = useFrontendConfig()
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

  const displayName = getBuiltinCapabilityDisplayName(approval.capabilityId, t)
  const reason = toSafeMcpDisplayText(approval.reason, 4096)

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={!canApprove}
      approveLabel={t('agent.approval.dialog.approve')}
      ariaBusy={isSubmitting}
      details={
        <dl className="agent-builtin-capability-approval__details">
          <div>
            <dt>{t('agent.builtinCapability.approval.reason')}</dt>
            <dd>{reason}</dd>
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
          ? t('agent.builtinCapability.approval.expired')
          : t('agent.builtinCapability.approval.taskGrantHint')
      }
      policyTone={expired ? 'danger' : 'default'}
      rejectDisabled={!canReject}
      rejectLabel={t('agent.approval.dialog.reject')}
      rejectMessage={rejectMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      request={formatTranslation(t, 'agent.builtinCapability.approval.title', {
        capability: displayName
      })}
    />
  )
}
