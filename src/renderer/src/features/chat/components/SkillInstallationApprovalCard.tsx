import { useEffect, useState } from 'react'
import type { AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import { ApprovalDialogShell } from './ApprovalDialogShell'
import {
  SkillInstallationPreviewDetails,
  skillInstallationPlainText
} from './SkillInstallationPreviewDetails'
import {
  resetApprovalSubmissionOnFailure,
  type ApprovalSubmissionResult
} from './approvalSubmission'

type SkillInstallationAction = Extract<AgentProposedAction, { type: 'skill_installation' }>

interface SkillInstallationApprovalCardProps {
  action: SkillInstallationAction
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => ApprovalSubmissionResult
  onReject?: (
    messageId: string,
    action: AgentProposedAction,
    message?: string
  ) => ApprovalSubmissionResult
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
  const preview = action.installation.preview

  useEffect(() => {
    setIsSubmitting(false)
    setRejectMessage('')
  }, [action.installation.id, messageId])

  const submit = (operation: () => ApprovalSubmissionResult) => {
    if (isSubmitting) return
    setIsSubmitting(true)
    resetApprovalSubmissionOnFailure(operation(), () => setIsSubmitting(false))
  }

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={!onApprove}
      approveLabel={t('agent.skillInstallation.approve')}
      details={<SkillInstallationPreviewDetails preview={preview} />}
      isSubmitting={isSubmitting}
      onApprove={() => submit(() => onApprove?.(messageId, action))}
      onReject={(message) =>
        submit(() => onReject?.(messageId, action, message.trim() || undefined))
      }
      onRejectMessageChange={setRejectMessage}
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
