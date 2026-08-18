import { useEffect, useState } from 'react'
import type { AgentMcpToolInvocationState, AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../config/translationFormat'
import { ApprovalDialogShell } from './ApprovalDialogShell'
import { BuiltinCapabilityActivationApprovalCard } from './BuiltinCapabilityActivationApprovalCard'
import { McpToolApprovalCard } from './McpToolApprovalCard'
import { SkillInstallationApprovalCard } from './SkillInstallationApprovalCard'
import { formatToolDetails, getToolDisplayName } from './toolActivities/toolActivityUtils'

type StandardAgentProposedAction = Exclude<
  AgentProposedAction,
  { type: 'mcp_tool_call' | 'builtin_capability_activation' | 'skill_installation' }
>

interface AgentApprovalDialogTarget {
  action: AgentProposedAction
  messageId: string
}

interface AgentApprovalDialogProps {
  allowRememberForRun?: boolean
  target: AgentApprovalDialogTarget
  mcpInvocationState?: AgentMcpToolInvocationState
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    options?: { rememberForRun?: boolean }
  ) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

function getApprovalFallbackTitle(action: StandardAgentProposedAction, t: Translate) {
  if (action.type === 'command') return t('agent.approval.dialog.commandTitle')
  if (action.type === 'diff') return t('agent.approval.dialog.diffTitle')
  if (action.type === 'file_write') return t('agent.approval.dialog.fileWriteTitle')
  if (action.type === 'skill_materialization')
    return formatTranslation(t, 'agent.approval.dialog.toolTitle', {
      tool: getToolDisplayName('skills_materialize_resource', t)
    })
  if (action.type === 'skill_script')
    return formatTranslation(t, 'agent.approval.dialog.toolTitle', {
      tool: getToolDisplayName('skills_run_script', t)
    })
  if (action.type === 'office_operation') {
    const kind = action.officeOperation.prepared.request.documentKind
    const tool =
      kind === 'document'
        ? 'office_document'
        : kind === 'spreadsheet'
          ? 'office_spreadsheet'
          : 'office_presentation'
    return formatTranslation(t, 'agent.approval.dialog.toolTitle', {
      tool: getToolDisplayName(tool, t)
    })
  }
  return formatTranslation(t, 'agent.approval.dialog.toolTitle', {
    tool: getToolDisplayName(action.call.tool, t)
  })
}

function getApprovalRequest(action: StandardAgentProposedAction, t: Translate) {
  if (action.type === 'diff') return action.diff.summary ?? action.diff.patch
  if (action.type === 'file_write')
    return action.fileWrite.summary ?? getApprovalFallbackTitle(action, t)
  if (action.type === 'command') return action.command.reason ?? getApprovalFallbackTitle(action, t)
  if (action.type === 'skill_materialization')
    return action.materialization.reason ?? getApprovalFallbackTitle(action, t)
  if (action.type === 'skill_script')
    return action.script.reason ?? getApprovalFallbackTitle(action, t)
  if (action.type === 'office_operation')
    return action.officeOperation.reason ?? getApprovalFallbackTitle(action, t)
  return action.call.reason ?? formatToolDetails(action.call.args)
}

function getApprovalCode(action: StandardAgentProposedAction) {
  if (action.type === 'command') return action.command.command
  if (action.type === 'diff') return action.diff.filePath
  if (action.type === 'file_write') return action.fileWrite.filePath
  if (action.type === 'skill_materialization') return action.materialization.destination
  if (action.type === 'skill_script')
    return JSON.stringify(
      {
        scriptUri: action.script.scriptUri,
        skillId: action.script.skillId,
        skillRevision: action.script.skillRevision,
        resourcePath: action.script.resourcePath,
        resourceDigest: action.script.resourceDigest,
        interpreter: action.script.interpreter,
        args: action.script.args,
        requirements: action.script.requirements,
        timeoutMs: action.script.timeoutMs,
        preflight: action.script.preflight
      },
      null,
      2
    )
  if (action.type === 'office_operation') {
    // Office approval only exposes user-facing logical paths. Engine, staging, normalized
    // paths and frozen execution metadata remain implementation details.
    return [
      ...new Set(
        action.officeOperation.prepared.paths.map((path) => path.logicalPath.trim()).filter(Boolean)
      )
    ].join('\n')
  }
  return action.call.tool
}

function getApprovalPolicyHint(action: StandardAgentProposedAction, t: Translate) {
  if (action.type === 'command') return t('agent.approval.dialog.commandPolicyHint')
  if (action.type === 'diff') return t('agent.approval.dialog.diffPolicyHint')
  if (action.type === 'file_write') return t('agent.approval.dialog.diffPolicyHint')
  if (action.type === 'skill_materialization') return t('agent.approval.dialog.diffPolicyHint')
  if (action.type === 'skill_script') return t('agent.approval.dialog.commandPolicyHint')
  if (action.type === 'office_operation') return t('agent.approval.dialog.diffPolicyHint')
  return t('agent.approval.dialog.toolPolicyHint')
}

function getRememberCommandPrefix(action: StandardAgentProposedAction) {
  if (action.type !== 'command') return ''
  return action.command.command.trim()
}

function canRememberForRun(action: StandardAgentProposedAction) {
  return action.type === 'command' || action.type === 'diff' || action.type === 'file_write'
}

interface StandardAgentApprovalDialogProps {
  action: StandardAgentProposedAction
  allowRememberForRun: boolean
  messageId: string
  onApprove?: AgentApprovalDialogProps['onApprove']
  onReject?: AgentApprovalDialogProps['onReject']
}

function StandardAgentApprovalDialog({
  action,
  allowRememberForRun,
  messageId,
  onApprove,
  onReject
}: StandardAgentApprovalDialogProps) {
  const { t } = useFrontendConfig()
  const [rejectMessage, setRejectMessage] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const request = getApprovalRequest(action, t)
  const code = getApprovalCode(action)
  const codeMultiline =
    action.type === 'skill_script' ||
    action.type === 'office_operation' ||
    (action.type === 'command' && /[\r\n]/u.test(code))
  const policyHint = getApprovalPolicyHint(action, t)
  const rememberPrefix = getRememberCommandPrefix(action)
  const showRememberChoice = allowRememberForRun && canRememberForRun(action)

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
    <ApprovalDialogShell
      approvalKind="standard"
      approveLabel={t('agent.approval.dialog.approve')}
      code={code}
      codeMultiline={codeMultiline}
      isSubmitting={isSubmitting}
      onApprove={() => approve(false)}
      onReject={reject}
      onRejectMessageChange={setRejectMessage}
      policyHint={policyHint}
      rejectLabel={t('agent.approval.dialog.reject')}
      rejectMessage={rejectMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      rememberChoice={
        showRememberChoice
          ? {
              content: (
                <span className="agent-approval-dialog__choice-text">
                  {action.type === 'diff' || action.type === 'file_write'
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
              ),
              onSelect: () => approve(true)
            }
          : undefined
      }
      request={request || getApprovalFallbackTitle(action, t)}
    />
  )
}

export function AgentApprovalDialog({
  allowRememberForRun = true,
  target,
  mcpInvocationState,
  onApprove,
  onCancel,
  onReject
}: AgentApprovalDialogProps) {
  if (target.action.type === 'mcp_tool_call') {
    return (
      <McpToolApprovalCard
        action={target.action}
        invocationState={mcpInvocationState}
        messageId={target.messageId}
        onApprove={onApprove}
        onCancel={onCancel}
        onReject={onReject}
      />
    )
  }

  if (target.action.type === 'builtin_capability_activation') {
    return (
      <BuiltinCapabilityActivationApprovalCard
        action={target.action}
        messageId={target.messageId}
        onApprove={onApprove}
        onCancel={onCancel}
        onReject={onReject}
      />
    )
  }

  if (target.action.type === 'skill_installation') {
    return (
      <SkillInstallationApprovalCard
        action={target.action}
        messageId={target.messageId}
        onApprove={onApprove}
        onReject={onReject}
      />
    )
  }

  return (
    <StandardAgentApprovalDialog
      action={target.action}
      allowRememberForRun={allowRememberForRun}
      messageId={target.messageId}
      onApprove={onApprove}
      onReject={onReject}
    />
  )
}
