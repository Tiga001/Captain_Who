import { useEffect, useRef, useState } from 'react'
import type {
  AgentMcpToolApproval,
  AgentMcpToolInvocationState,
  AgentProposedAction
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { toSafeMcpDisplayText } from '../../mcp/mcpSafeDisplay'
import { ApprovalDialogShell } from './ApprovalDialogShell'

type McpToolCallAction = Extract<AgentProposedAction, { type: 'mcp_tool_call' }>

interface McpToolApprovalCardProps {
  action: McpToolCallAction
  invocationState?: AgentMcpToolInvocationState
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

const APPROVABLE_INVOCATION_STATES = new Set<AgentMcpToolInvocationState>(['pending_approval'])

const stateTranslationKeys = {
  pending_approval: 'agent.mcp.approval.state.pendingApproval',
  approved: 'agent.mcp.approval.state.approved',
  dispatching: 'agent.mcp.approval.state.dispatching',
  running: 'agent.mcp.approval.state.running',
  completed: 'agent.mcp.approval.state.completed',
  failed: 'agent.mcp.approval.state.failed',
  cancelled: 'agent.mcp.approval.state.cancelled',
  rejected: 'agent.mcp.approval.state.rejected',
  expired: 'agent.mcp.approval.state.expired',
  payload_unavailable: 'agent.mcp.approval.state.payloadUnavailable',
  policy_denied: 'agent.mcp.approval.state.policyDenied',
  outcome_unknown: 'agent.mcp.approval.state.outcomeUnknown'
} as const

function getBlockingState(
  approval: AgentMcpToolApproval,
  invocationState: AgentMcpToolInvocationState | undefined,
  expired: boolean
): AgentMcpToolInvocationState | undefined {
  if (expired) return 'expired'
  if (approval.approvalMode !== 'prompt') return 'policy_denied'
  if (invocationState && !APPROVABLE_INVOCATION_STATES.has(invocationState)) {
    return invocationState
  }
  return undefined
}

function getMcpApprovalReason(approval: AgentMcpToolApproval, fallback: string): string {
  // `displayReason`/`reason` are bounded model-authored plain-text fields. They remain untrusted
  // and may contain user-provided sensitive text. Until the Host supplies one, `call.reason` is
  // the only typed fallback; raw arguments are never inspected to synthesize a reason.
  const summaryReason = approval.summary.displayReason
  const candidate =
    typeof summaryReason === 'string' && summaryReason.trim() ? summaryReason : approval.call.reason
  const safeReason =
    typeof candidate === 'string' ? toSafeMcpDisplayText(candidate.trim(), 512) : ''
  return safeReason || fallback
}

export function McpToolApprovalCard({
  action,
  invocationState,
  messageId,
  onApprove,
  onCancel,
  onReject
}: McpToolApprovalCardProps) {
  const { t } = useFrontendConfig()
  const submittingRef = useRef(false)
  const { approval } = action
  const [expired, setExpired] = useState(() => Date.now() >= approval.expiresAt)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [rejectionMessage, setRejectionMessage] = useState('')

  useEffect(() => {
    submittingRef.current = false
    setIsSubmitting(false)
    setRejectionMessage('')

    const remainingMs = approval.expiresAt - Date.now()
    if (remainingMs <= 0) {
      setExpired(true)
      return
    }

    setExpired(false)
    const timeoutId = window.setTimeout(() => setExpired(true), remainingMs)
    return () => window.clearTimeout(timeoutId)
  }, [approval.expiresAt, approval.identity.actionId])

  const currentState = invocationState ?? 'pending_approval'
  const blockingState = getBlockingState(approval, invocationState, expired)
  const canApprove =
    !blockingState && approval.approvalMode === 'prompt' && Boolean(onApprove) && !isSubmitting
  const canReject = Boolean(onReject) && !isSubmitting && currentState === 'pending_approval'
  const canCancel = Boolean(onCancel) && !isSubmitting && currentState === 'pending_approval'

  const submitOnce = (submit: (() => void) | undefined) => {
    if (!submit || submittingRef.current) return
    submittingRef.current = true
    setIsSubmitting(true)
    submit()
  }

  const approve = () => {
    if (!canApprove) return
    submitOnce(() => onApprove?.(messageId, action))
  }

  const reject = (message: string) => {
    if (!canReject) return
    const guidance = message.trim()
    submitOnce(() => onReject?.(messageId, action, guidance || undefined))
  }

  const cancel = () => {
    if (!canCancel) return
    submitOnce(() => onCancel?.(messageId, action))
  }

  const safeServerName = toSafeMcpDisplayText(approval.summary.serverDisplayName, 256)
  const safeToolName = toSafeMcpDisplayText(approval.summary.rawToolName, 256)

  return (
    <ApprovalDialogShell
      approvalKind="mcp"
      approveDisabled={!canApprove}
      approveLabel={t('agent.approval.dialog.approve')}
      ariaBusy={isSubmitting}
      code={
        <>
          {t('agent.mcp.approval.serverLabel')}: {safeServerName}
          {' · '}
          {t('agent.mcp.approval.rawToolLabel')}: {safeToolName}
        </>
      }
      isSubmitting={isSubmitting}
      onApprove={approve}
      onCancel={canCancel ? cancel : undefined}
      onReject={reject}
      onRejectMessageChange={setRejectionMessage}
      policyHint={blockingState ? t(stateTranslationKeys[blockingState]) : undefined}
      policyTone={blockingState ? 'danger' : 'default'}
      rejectDisabled={!canReject}
      rejectLabel={t('agent.approval.dialog.reject')}
      rejectMessage={rejectionMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      request={getMcpApprovalReason(approval, t('agent.mcp.approval.title'))}
    />
  )
}
