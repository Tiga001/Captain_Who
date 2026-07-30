import { useId, type ReactNode } from 'react'
import { CornerDownLeft, PencilLine } from 'lucide-react'

interface ApprovalDialogShellRememberChoice {
  content: ReactNode
  disabled?: boolean
  onSelect: () => void
}

interface ApprovalDialogShellProps {
  approvalKind: 'mcp' | 'standard'
  approveDisabled?: boolean
  approveLabel: string
  ariaBusy?: boolean
  code?: ReactNode
  codeMultiline?: boolean
  isSubmitting: boolean
  onApprove: () => void
  onCancel?: () => void
  onReject: (message: string) => void
  onRejectMessageChange: (message: string) => void
  policyHint?: string
  policyTone?: 'danger' | 'default'
  rejectDisabled?: boolean
  rejectLabel: string
  rejectMessage: string
  rejectPlaceholder: string
  rememberChoice?: ApprovalDialogShellRememberChoice
  request: string
}

/**
 * Shared presentation shell for every Agent approval. It deliberately receives only render-safe
 * text/React nodes; routing identities and raw Tool arguments never enter this component.
 */
export function ApprovalDialogShell({
  approvalKind,
  approveDisabled = false,
  approveLabel,
  ariaBusy = false,
  code,
  codeMultiline = false,
  isSubmitting,
  onApprove,
  onCancel,
  onReject,
  onRejectMessageChange,
  policyHint,
  policyTone = 'default',
  rejectDisabled = false,
  rejectLabel,
  rejectMessage,
  rejectPlaceholder,
  rememberChoice,
  request
}: ApprovalDialogShellProps) {
  const titleId = useId()

  return (
    <section
      aria-busy={ariaBusy}
      aria-keyshortcuts={onCancel ? 'Escape' : undefined}
      aria-labelledby={titleId}
      className="agent-approval-dialog"
      data-approval-kind={approvalKind}
      onKeyDown={(event) => {
        if (event.key !== 'Escape' || !onCancel || isSubmitting) return
        event.preventDefault()
        event.stopPropagation()
        onCancel()
      }}
      role="dialog"
    >
      <h2 className="agent-approval-dialog__request" id={titleId}>
        {request}
      </h2>

      {code ? (
        <code
          className="agent-approval-dialog__command"
          data-multiline={codeMultiline ? 'true' : undefined}
        >
          {code}
        </code>
      ) : null}

      {policyHint ? (
        <p
          className="agent-approval-dialog__policy"
          data-tone={policyTone === 'danger' ? 'danger' : undefined}
          role={policyTone === 'danger' ? 'alert' : undefined}
        >
          {policyHint}
        </p>
      ) : null}

      <button
        className="agent-approval-dialog__choice"
        data-choice="primary"
        disabled={isSubmitting || approveDisabled}
        onClick={onApprove}
        type="button"
      >
        <span className="agent-approval-dialog__index">1</span>
        <span>{approveLabel}</span>
      </button>

      {rememberChoice ? (
        <button
          className="agent-approval-dialog__choice"
          data-choice="remember"
          disabled={isSubmitting || rememberChoice.disabled}
          onClick={rememberChoice.onSelect}
          type="button"
        >
          <span className="agent-approval-dialog__index">2</span>
          {rememberChoice.content}
        </button>
      ) : null}

      <div className="agent-approval-dialog__reject-row">
        <span className="agent-approval-dialog__reject-icon" aria-hidden="true">
          <PencilLine />
        </span>
        <input
          aria-label={rejectPlaceholder}
          disabled={isSubmitting || rejectDisabled}
          onChange={(event) => onRejectMessageChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
              event.preventDefault()
              onReject(rejectMessage)
            }
          }}
          placeholder={rejectPlaceholder}
          value={rejectMessage}
        />
        <button
          className="agent-approval-dialog__reject"
          disabled={isSubmitting || rejectDisabled}
          onClick={() => onReject(rejectMessage)}
          type="button"
        >
          {rejectLabel}
          <CornerDownLeft aria-hidden="true" />
        </button>
      </div>
    </section>
  )
}
