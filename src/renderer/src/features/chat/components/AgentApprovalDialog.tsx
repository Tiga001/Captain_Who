// Renderer UI.
import { useEffect, useId, useState } from 'react'
import { CornerDownLeft, PencilLine } from 'lucide-react'
import type { AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../config/translationFormat'
import { formatToolDetails, getToolDisplayName } from './toolActivities/toolActivityUtils'

export interface AgentApprovalDialogTarget {
  action: AgentProposedAction
  messageId: string
}

interface AgentApprovalDialogProps {
  target: AgentApprovalDialogTarget
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    options?: { rememberForRun?: boolean }
  ) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

function getApprovalFallbackTitle(action: AgentProposedAction, t: Translate) {
  if (action.type === 'command') return t('agent.approval.dialog.commandTitle')
  if (action.type === 'diff') return t('agent.approval.dialog.diffTitle')
  return formatTranslation(t, 'agent.approval.dialog.toolTitle', {
    tool: getToolDisplayName(action.call.tool, t)
  })
}

function getApprovalRequest(action: AgentProposedAction, t: Translate) {
  if (action.type === 'diff') return action.diff.summary ?? action.diff.patch
  if (action.type === 'command') return action.command.reason ?? getApprovalFallbackTitle(action, t)
  return action.call.reason ?? formatToolDetails(action.call.args)
}

function getApprovalCode(action: AgentProposedAction) {
  if (action.type === 'command') return action.command.command
  if (action.type === 'diff') return action.diff.filePath
  return action.call.tool
}

function getApprovalPolicyHint(action: AgentProposedAction, t: Translate) {
  if (action.type === 'command') return t('agent.approval.dialog.commandPolicyHint')
  if (action.type === 'diff') return t('agent.approval.dialog.diffPolicyHint')
  return t('agent.approval.dialog.toolPolicyHint')
}

function getRememberCommandPrefix(action: AgentProposedAction) {
  if (action.type !== 'command') return ''
  return action.command.command.trim()
}

function canRememberForRun(action: AgentProposedAction) {
  return action.type === 'command' || action.type === 'diff'
}

export function AgentApprovalDialog({ target, onApprove, onReject }: AgentApprovalDialogProps) {
  const { t } = useFrontendConfig()
  const titleId = useId()
  const [rejectMessage, setRejectMessage] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const { action, messageId } = target
  const request = getApprovalRequest(action, t)
  const code = getApprovalCode(action)
  const policyHint = getApprovalPolicyHint(action, t)
  const rememberPrefix = getRememberCommandPrefix(action)
  const showRememberChoice = canRememberForRun(action)

  useEffect(() => {
    setRejectMessage('')
    setIsSubmitting(false)
  }, [action, messageId])

  const approve = (rememberForRun = false) => {
    if (isSubmitting) return
    setIsSubmitting(true)
    onApprove?.(messageId, action, { rememberForRun })
  }

  const reject = () => {
    if (isSubmitting) return
    setIsSubmitting(true)
    onReject?.(messageId, action, rejectMessage)
  }

  return (
    <section aria-labelledby={titleId} className="agent-approval-dialog" role="dialog">
      <h2 className="agent-approval-dialog__request" id={titleId}>
        {request || getApprovalFallbackTitle(action, t)}
      </h2>

      {code ? <code className="agent-approval-dialog__command">{code}</code> : null}
      <p className="agent-approval-dialog__policy">{policyHint}</p>

      <button
        className="agent-approval-dialog__choice"
        data-choice="primary"
        disabled={isSubmitting}
        onClick={() => approve(false)}
        type="button"
      >
        <span className="agent-approval-dialog__index">1</span>
        <span>{t('agent.approval.dialog.approve')}</span>
      </button>

      {showRememberChoice ? (
        <button
          className="agent-approval-dialog__choice"
          data-choice="remember"
          disabled={isSubmitting}
          onClick={() => approve(true)}
          type="button"
        >
          <span className="agent-approval-dialog__index">2</span>
          <span className="agent-approval-dialog__choice-text">
            {action.type === 'diff'
              ? t('agent.approval.dialog.approvePatchRemember')
              : t('agent.approval.dialog.approveRemember')}
            {rememberPrefix ? (
              <small>
                {formatTranslation(t, 'agent.approval.dialog.rememberPrefix', {
                  prefix: rememberPrefix
                })}
              </small>
            ) : null}
          </span>
        </button>
      ) : null}

      <div className="agent-approval-dialog__reject-row">
        <span className="agent-approval-dialog__reject-icon" aria-hidden="true">
          <PencilLine />
        </span>
        <input
          aria-label={t('agent.approval.dialog.rejectPlaceholder')}
          disabled={isSubmitting}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
              event.preventDefault()
              reject()
            }
          }}
          onChange={(event) => setRejectMessage(event.target.value)}
          placeholder={t('agent.approval.dialog.rejectPlaceholder')}
          value={rejectMessage}
        />
        <button
          className="agent-approval-dialog__reject"
          disabled={isSubmitting}
          onClick={reject}
          type="button"
        >
          {t('agent.approval.dialog.reject')}
          <CornerDownLeft aria-hidden="true" />
        </button>
      </div>
    </section>
  )
}
