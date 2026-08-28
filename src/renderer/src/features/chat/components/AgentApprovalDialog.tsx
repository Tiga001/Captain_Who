import { useEffect, useState } from 'react'
import type { AgentMcpToolInvocationState, AgentProposedAction } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../../config/translationFormat'
import { ApprovalDialogShell } from './ApprovalDialogShell'
import { BrowserRiskApprovalCard } from './BrowserRiskApprovalCard'
import { BuiltinCapabilityActivationApprovalCard } from './BuiltinCapabilityActivationApprovalCard'
import { BuiltinMcpToolApprovalCard } from './BuiltinMcpToolApprovalCard'
import { McpToolApprovalCard } from './McpToolApprovalCard'
import { SkillInstallationApprovalCard } from './SkillInstallationApprovalCard'
import {
  resetApprovalSubmissionOnFailure,
  type ApprovalSubmissionResult
} from './approvalSubmission'
import { formatToolDetails, getToolDisplayName } from './toolActivities/toolActivityUtils'
import { getAgentFileChangeDiff } from '../../agent/agentClient'

const FILE_CHANGE_DIFF_PAGE_CHARS = 50_000
const MAX_FILE_CHANGE_DIFF_PAGES = 256
const MAX_FILE_CHANGE_DIFF_CHARS = 8 * 1024 * 1024

type StandardAgentProposedAction = Exclude<
  AgentProposedAction,
  {
    type:
      | 'mcp_tool_call'
      | 'builtin_capability_activation'
      | 'builtin_mcp_tool_approval'
      | 'browser_risk_approval'
      | 'skill_installation'
  }
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
  ) => ApprovalSubmissionResult
  onCancel?: (messageId: string, action: AgentProposedAction) => ApprovalSubmissionResult
  onReject?: (
    messageId: string,
    action: AgentProposedAction,
    message?: string
  ) => ApprovalSubmissionResult
}

function getApprovalFallbackTitle(action: StandardAgentProposedAction, t: Translate) {
  if (action.type === 'command') return t('agent.approval.dialog.commandTitle')
  if (action.type === 'file_change') return t('agent.approval.dialog.diffTitle')
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
  if (action.type === 'file_change')
    return action.fileChange.summary ?? getApprovalFallbackTitle(action, t)
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
  if (action.type === 'file_change') return action.fileChange.filePath
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
  if (action.type === 'file_change') return t('agent.approval.dialog.diffPolicyHint')
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
  return action.type === 'command' || action.type === 'file_change'
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
  const [fileChangeDiff, setFileChangeDiff] = useState<{
    transactionId: string | null
    content: string
    error: string
    loading: boolean
  }>(() => {
    if (action.type !== 'file_change') {
      return { transactionId: null, content: '', error: '', loading: false }
    }
    return {
      transactionId: action.fileChange.transactionId,
      content: action.fileChange.inlineDiff?.patch ?? '',
      error: '',
      loading: action.fileChange.inlineDiff === null
    }
  })
  const request = getApprovalRequest(action, t)
  const code = getApprovalCode(action)
  const codeMultiline =
    action.type === 'skill_script' ||
    action.type === 'office_operation' ||
    (action.type === 'command' && /[\r\n]/u.test(code))
  const policyHint = getApprovalPolicyHint(action, t)
  const rememberPrefix = getRememberCommandPrefix(action)
  const showRememberChoice = allowRememberForRun && canRememberForRun(action)
  const fileChangeTransactionId =
    action.type === 'file_change' ? action.fileChange.transactionId : null
  const inlineFileChangeDiff =
    action.type === 'file_change' ? (action.fileChange.inlineDiff?.patch ?? null) : null
  const fileChangePreviewError = t('files.preview.error')

  useEffect(() => {
    setRejectMessage('')
    setIsSubmitting(false)
  }, [action, messageId])

  useEffect(() => {
    if (fileChangeTransactionId === null) {
      setFileChangeDiff({ transactionId: null, content: '', error: '', loading: false })
      return
    }
    if (inlineFileChangeDiff !== null) {
      setFileChangeDiff({
        transactionId: fileChangeTransactionId,
        content: inlineFileChangeDiff,
        error: '',
        loading: false
      })
      return
    }

    let cancelled = false
    setFileChangeDiff({
      transactionId: fileChangeTransactionId,
      content: '',
      error: '',
      loading: true
    })
    void (async () => {
      const chunks: string[] = []
      let offset = 0
      let loadedChars = 0
      const visitedOffsets = new Set<number>()
      while (true) {
        if (visitedOffsets.size >= MAX_FILE_CHANGE_DIFF_PAGES) {
          throw new Error('FileChange diff page chain exceeded its bound')
        }
        if (visitedOffsets.has(offset)) throw new Error('FileChange diff page chain repeated')
        visitedOffsets.add(offset)
        const page = await getAgentFileChangeDiff(
          fileChangeTransactionId,
          offset,
          FILE_CHANGE_DIFF_PAGE_CHARS
        )
        if (page.transactionId !== fileChangeTransactionId || page.offset !== offset) {
          throw new Error('FileChange diff page identity mismatch')
        }
        loadedChars += page.patch.length
        if (loadedChars > MAX_FILE_CHANGE_DIFF_CHARS) {
          throw new Error('FileChange diff exceeded its Renderer review bound')
        }
        chunks.push(page.patch)
        if (!page.truncated) {
          if (page.nextOffset !== null) {
            throw new Error('FileChange terminal diff page has a continuation')
          }
          break
        }
        if (page.nextOffset === null || page.nextOffset <= offset) {
          throw new Error('FileChange diff page chain is incomplete')
        }
        offset = page.nextOffset
      }
      if (!cancelled) {
        setFileChangeDiff({
          transactionId: fileChangeTransactionId,
          content: chunks.join(''),
          error: '',
          loading: false
        })
      }
    })().catch(() => {
      if (!cancelled) {
        setFileChangeDiff({
          transactionId: fileChangeTransactionId,
          content: '',
          error: fileChangePreviewError,
          loading: false
        })
      }
    })
    return () => {
      cancelled = true
    }
  }, [fileChangePreviewError, fileChangeTransactionId, inlineFileChangeDiff])

  const fileChangeDiffCurrent =
    action.type === 'file_change' &&
    fileChangeDiff.transactionId === action.fileChange.transactionId
  const fileChangeDiffReady =
    action.type !== 'file_change' ||
    (fileChangeDiffCurrent && !fileChangeDiff.loading && !fileChangeDiff.error)

  const approve = (rememberForRun = false) => {
    if (isSubmitting || !fileChangeDiffReady) return
    setIsSubmitting(true)
    resetApprovalSubmissionOnFailure(onApprove?.(messageId, action, { rememberForRun }), () =>
      setIsSubmitting(false)
    )
  }

  const reject = () => {
    if (isSubmitting) return
    setIsSubmitting(true)
    resetApprovalSubmissionOnFailure(onReject?.(messageId, action, rejectMessage), () =>
      setIsSubmitting(false)
    )
  }

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={!fileChangeDiffReady}
      approveLabel={t('agent.approval.dialog.approve')}
      code={code}
      codeMultiline={codeMultiline}
      details={
        action.type === 'file_change' ? (
          !fileChangeDiffCurrent || fileChangeDiff.loading ? (
            <p>{t('files.preview.loading')}</p>
          ) : fileChangeDiff.error ? (
            <p role="alert">{fileChangeDiff.error}</p>
          ) : (
            <pre>{fileChangeDiff.content}</pre>
          )
        ) : undefined
      }
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
                  {action.type === 'file_change'
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
              disabled: !fileChangeDiffReady,
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

  if (target.action.type === 'browser_risk_approval') {
    return (
      <BrowserRiskApprovalCard
        action={target.action}
        messageId={target.messageId}
        onApprove={onApprove}
        onCancel={onCancel}
        onReject={onReject}
      />
    )
  }

  if (target.action.type === 'builtin_mcp_tool_approval') {
    return (
      <BuiltinMcpToolApprovalCard
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
