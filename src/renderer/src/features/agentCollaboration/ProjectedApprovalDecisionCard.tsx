import { useState } from 'react'
import type {
  CollaborationApprovalDecisionResult,
  CollaborationApprovalProjection
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { AgentApprovalDialog } from '../chat/components/AgentApprovalDialog'
import '../chat/ChatConversationPage.approvals.css'
import './ProjectedApprovalDecisionCard.css'

export type CollaborationApprovalDecision = 'approve' | 'reject' | 'cancel'

export type CollaborationApprovalDecisionHandler = (
  approvalId: string,
  decision: CollaborationApprovalDecision,
  message: string | null
) => Promise<CollaborationApprovalDecisionResult>

export function ProjectedApprovalDecisionCard({
  approval,
  onDecision
}: {
  approval: CollaborationApprovalProjection
  onDecision: CollaborationApprovalDecisionHandler
}) {
  const { t } = useFrontendConfig()
  const [error, setError] = useState<string | null>(null)

  const decide = async (decision: CollaborationApprovalDecision, message: string | null = null) => {
    setError(null)
    try {
      const result = await onDecision(approval.approvalId, decision, message)
      if (
        (!result.accepted && result.status === 'pending') ||
        (!result.accepted && !result.alreadySettled)
      ) {
        throw new Error(t('collaboration.approval.decisionNotAccepted'))
      }
      // Keep accepted and already-settled decisions latched until their durable projection
      // removes the card. On failure, reset submission without discarding rejection guidance.
      return true
    } catch (decisionError) {
      setError(decisionError instanceof Error ? decisionError.message : String(decisionError))
      return false
    }
  }

  return (
    <div className="collaboration-approval__decision" data-approval-id={approval.approvalId}>
      <AgentApprovalDialog
        allowRunScopedApproval={false}
        observerRootConversationId={approval.rootConversationId}
        onApprove={() => decide('approve')}
        onCancel={() => decide('cancel')}
        onReject={(_messageId, _action, message) => decide('reject', message?.trim() || null)}
        target={{ action: approval.action, messageId: approval.approvalId }}
      />
      {error && (
        <p className="collaboration-approval__error" role="alert">
          {formatTranslation(t, 'collaboration.approval.decisionFailed', { error })}
        </p>
      )}
    </div>
  )
}
