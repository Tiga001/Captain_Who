import {
  Ban,
  CheckCircle2,
  CircleHelp,
  LoaderCircle,
  ShieldAlert,
  TriangleAlert,
  XCircle
} from 'lucide-react'
import { useMemo, useState } from 'react'
import type {
  CollaborationApprovalDecisionResult,
  CollaborationApprovalProjection,
  CollaborationApprovalStatus
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import { AgentApprovalDialog } from '../chat/components/AgentApprovalDialog'
import '../chat/ChatConversationPage.approvals.css'
import './CollaborationApprovalPanel.css'

export type CollaborationApprovalDecision = 'approve' | 'reject' | 'cancel'

export interface CollaborationApprovalPanelBaseProps {
  approvals: readonly CollaborationApprovalProjection[]
  loadError?: string | null
  onOpenAgent?: (agentId: string) => void
}

export type CollaborationApprovalPanelProps = CollaborationApprovalPanelBaseProps &
  (
    | {
        mode: 'interactive'
        onDecision: (
          approvalId: string,
          decision: CollaborationApprovalDecision,
          message: string | null
        ) => Promise<CollaborationApprovalDecisionResult>
        onRetryLoad?: () => void
      }
    | {
        mode: 'observer'
        onDecision?: never
        onRetryLoad?: never
      }
  )

const STATUS_PRIORITY: Record<CollaborationApprovalStatus, number> = {
  pending: 0,
  approved: 1,
  executing: 2,
  rejected: 3,
  cancelled: 4,
  failed: 5,
  expired: 6,
  interrupted: 7,
  completed: 8
}

function approvalStatusPresentation(status: CollaborationApprovalStatus, t: Translate) {
  switch (status) {
    case 'pending':
      return {
        icon: ShieldAlert,
        label: t('collaboration.approval.status.pending'),
        tone: 'warning'
      } as const
    case 'approved':
      return {
        icon: CheckCircle2,
        label: t('collaboration.approval.status.approved'),
        tone: 'active'
      } as const
    case 'executing':
      return {
        icon: LoaderCircle,
        label: t('collaboration.approval.status.executing'),
        tone: 'active'
      } as const
    case 'rejected':
      return {
        icon: XCircle,
        label: t('collaboration.approval.status.rejected'),
        tone: 'muted'
      } as const
    case 'cancelled':
      return {
        icon: Ban,
        label: t('collaboration.approval.status.cancelled'),
        tone: 'muted'
      } as const
    case 'completed':
      return {
        icon: CheckCircle2,
        label: t('collaboration.approval.status.completed'),
        tone: 'success'
      } as const
    case 'failed':
      return {
        icon: TriangleAlert,
        label: t('collaboration.approval.status.failed'),
        tone: 'danger'
      } as const
    case 'expired':
      return {
        icon: CircleHelp,
        label: t('collaboration.approval.status.expired'),
        tone: 'muted'
      } as const
    case 'interrupted':
      return {
        icon: Ban,
        label: t('collaboration.approval.status.interrupted'),
        tone: 'muted'
      } as const
  }
}

function ProjectedApprovalDecisionCard({
  approval,
  onDecision
}: {
  approval: CollaborationApprovalProjection
  onDecision: Extract<CollaborationApprovalPanelProps, { mode: 'interactive' }>['onDecision']
}) {
  const { t } = useFrontendConfig()
  const [error, setError] = useState<string | null>(null)
  const [attempt, setAttempt] = useState(0)

  const decide = async (decision: CollaborationApprovalDecision, message: string | null = null) => {
    setError(null)
    try {
      const result = await onDecision(approval.approvalId, decision, message)
      if (!result.accepted && result.status === 'pending') {
        throw new Error(t('collaboration.approval.decisionNotAccepted'))
      }
      if (!result.accepted && !result.alreadySettled) {
        throw new Error(t('collaboration.approval.decisionNotAccepted'))
      }
      // accepted=true is the only new decision acknowledgement that stays latched. An
      // already-settled response is also left closed because its authoritative non-pending status
      // is projected by the controller; it must never invite a duplicate decision.
    } catch (decisionError) {
      setError(decisionError instanceof Error ? decisionError.message : String(decisionError))
      // AgentApprovalDialog intentionally latches while a root decision is being routed. Remount
      // after a rejected or unaccepted request so the user can retry. Accepted decisions wait for
      // the durable approval projection instead of optimistically reopening the action.
      setAttempt((current) => current + 1)
    }
  }

  return (
    <div className="collaboration-approval__decision">
      <AgentApprovalDialog
        allowRememberForRun={false}
        key={`${approval.approvalId}:${attempt}`}
        observerRootConversationId={approval.rootConversationId}
        onApprove={() => void decide('approve')}
        onCancel={() => void decide('cancel')}
        onReject={(_messageId, _action, message) => void decide('reject', message?.trim() || null)}
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

/** Root-routed child approvals. Observer mode deliberately has no write-capable callbacks. */
export function CollaborationApprovalPanel(props: CollaborationApprovalPanelProps) {
  const { t } = useFrontendConfig()
  const approvals = useMemo(() => {
    const byApprovalId = new Map<string, CollaborationApprovalProjection>()
    for (const approval of props.approvals) {
      const current = byApprovalId.get(approval.approvalId)
      if (!current || approval.updatedAt >= current.updatedAt) {
        byApprovalId.set(approval.approvalId, approval)
      }
    }
    return [...byApprovalId.values()].sort((left, right) => {
      const priority = STATUS_PRIORITY[left.status] - STATUS_PRIORITY[right.status]
      if (priority !== 0) return priority
      if (left.updatedAt !== right.updatedAt) return right.updatedAt - left.updatedAt
      return left.approvalId.localeCompare(right.approvalId)
    })
  }, [props.approvals])

  if (approvals.length === 0 && !props.loadError) return null

  return (
    <section
      aria-label={t('collaboration.approval.title')}
      className="collaboration-approval"
      data-mode={props.mode}
      data-testid="collaboration-approval"
    >
      <h3>{t('collaboration.approval.title')}</h3>
      {props.loadError && (
        <div className="collaboration-approval__load-error" role="alert">
          <span>
            {formatTranslation(t, 'collaboration.approval.loadFailed', {
              error: props.loadError
            })}
          </span>
          {props.mode === 'interactive' && props.onRetryLoad && (
            <button onClick={props.onRetryLoad} type="button">
              {t('collaboration.approval.retry')}
            </button>
          )}
        </div>
      )}
      <div className="collaboration-approval__list">
        {approvals.map((approval) => {
          const status = approvalStatusPresentation(approval.status, t)
          const StatusIcon = status.icon
          const source = (
            <>
              <strong>{approval.sourceTaskPath}</strong>
              <span>{approval.toolName}</span>
            </>
          )
          return (
            <article
              className="collaboration-approval__item"
              data-approval-id={approval.approvalId}
              data-status={approval.status}
              key={approval.approvalId}
            >
              <header>
                {props.onOpenAgent ? (
                  <button
                    aria-label={formatTranslation(t, 'collaboration.approval.openAgent', {
                      task: approval.sourceTaskPath
                    })}
                    className="collaboration-approval__source"
                    onClick={() => props.onOpenAgent?.(approval.sourceAgentId)}
                    type="button"
                  >
                    {source}
                  </button>
                ) : (
                  <span className="collaboration-approval__source">{source}</span>
                )}
                <span className="collaboration-approval__status" data-tone={status.tone}>
                  <StatusIcon
                    aria-hidden="true"
                    className={approval.status === 'executing' ? 'is-spinning' : undefined}
                  />
                  {status.label}
                </span>
              </header>

              {props.mode === 'interactive' && approval.status === 'pending' ? (
                <ProjectedApprovalDecisionCard approval={approval} onDecision={props.onDecision} />
              ) : (
                <p className="collaboration-approval__readonly">
                  {props.mode === 'observer'
                    ? t('collaboration.approval.observerNotice')
                    : status.label}
                </p>
              )}
            </article>
          )
        })}
      </div>
    </section>
  )
}
