import { useEffect, useState } from 'react'
import type { AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import { ApprovalDialogShell } from './ApprovalDialogShell'
import {
  SkillInstallationPreviewDetails,
  skillInstallationPlainText
} from './SkillInstallationPreviewDetails'

type SkillInstallationAction = Extract<AgentProposedAction, { type: 'skill_installation' }>

interface SkillInstallationApprovalCardProps {
  action: SkillInstallationAction
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

export function SkillInstallationApprovalCard({
  action,
  messageId,
  onApprove,
  onReject
}: SkillInstallationApprovalCardProps) {
  const { t } = useFrontendConfig()
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [rejectMessage, setRejectMessage] = useState('')
  const [now, setNow] = useState(() => Date.now())
  const preview = action.installation.preview
  const expired = now >= action.installation.expiresAt

  useEffect(() => {
    setIsSubmitting(false)
    setRejectMessage('')
    setNow(Date.now())
  }, [action.installation.id, messageId])

  useEffect(() => {
    if (expired) return
    const delay = Math.max(0, action.installation.expiresAt - Date.now())
    const timer = window.setTimeout(() => setNow(Date.now()), delay + 10)
    return () => window.clearTimeout(timer)
  }, [action.installation.expiresAt, expired])

  const submit = (operation: () => void, allowExpired = false) => {
    if (isSubmitting || (expired && !allowExpired)) return
    setIsSubmitting(true)
    operation()
  }

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={expired || !onApprove}
      approveLabel={t('agent.skillInstallation.approve')}
      details={<SkillInstallationPreviewDetails preview={preview} />}
      isSubmitting={isSubmitting}
      onApprove={() => submit(() => onApprove?.(messageId, action))}
      onReject={(message) =>
        submit(() => onReject?.(messageId, action, message.trim() || undefined), true)
      }
      onRejectMessageChange={setRejectMessage}
      policyHint={expired ? t('agent.skillInstallation.expired') : undefined}
      policyTone={expired ? 'danger' : 'default'}
      rejectDisabled={!onReject}
      rejectLabel={t('agent.skillInstallation.reject')}
      rejectMessage={rejectMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      request={formatTranslation(t, 'agent.skillInstallation.title', {
        name: skillInstallationPlainText(preview.name, t('agent.skillInstallation.unnamed'))
      })}
    />
  )
}
