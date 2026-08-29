import { useEffect, useState } from 'react'
import type {
  AgentApprovalScope,
  AgentMcpToolInvocationState,
  AgentProposedAction
} from '@mycopilot/protocol'
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
  allowRunScopedApproval?: boolean
  observerRootConversationId?: string
  target: AgentApprovalDialogTarget
  mcpInvocationState?: AgentMcpToolInvocationState
  onApprove?: (
    messageId: string,
    action: AgentProposedAction,
    approvalScope: AgentApprovalScope
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
  if (action.type === 'file_change') return t('agent.approval.dialog.fileChangeTitle')
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
  if (action.type === 'file_change') return t('agent.approval.dialog.fileChangePolicyHint')
  if (action.type === 'skill_materialization') return t('agent.approval.dialog.toolPolicyHint')
  if (action.type === 'skill_script') return t('agent.approval.dialog.commandPolicyHint')
  if (action.type === 'office_operation') return t('agent.approval.dialog.toolPolicyHint')
  return t('agent.approval.dialog.toolPolicyHint')
}

function canApproveRemainingApplyPatchInRun(action: StandardAgentProposedAction) {
  return (
    action.type === 'file_change' &&
    (action.fileChange.operation === 'create' || action.fileChange.operation === 'update')
  )
}

interface StandardAgentApprovalDialogProps {
  action: StandardAgentProposedAction
  allowRunScopedApproval: boolean
  messageId: string
  observerRootConversationId?: string
  onApprove?: AgentApprovalDialogProps['onApprove']
  onReject?: AgentApprovalDialogProps['onReject']
}

function StandardAgentApprovalDialog({
  action,
  allowRunScopedApproval,
  messageId,
  observerRootConversationId,
  onApprove,
  onReject
}: StandardAgentApprovalDialogProps) {
  const { t } = useFrontendConfig()
  const [rejectMessage, setRejectMessage] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [fileChangeDiff, setFileChangeDiff] = useState<{
    transactionId: string | null
    pages: string[]
    pageIndex: number
    error: string
    loading: boolean
  }>(() => {
    if (action.type !== 'file_change') {
      return { transactionId: null, pages: [], pageIndex: 0, error: '', loading: false }
    }
    return {
      transactionId: action.fileChange.transactionId,
      pages: action.fileChange.inlineDiff ? [action.fileChange.inlineDiff.patch] : [],
      pageIndex: 0,
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
  const showRememberChoice = allowRunScopedApproval && canApproveRemainingApplyPatchInRun(action)
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
      setFileChangeDiff({
        transactionId: null,
        pages: [],
        pageIndex: 0,
        error: '',
        loading: false
      })
      return
    }
    if (inlineFileChangeDiff !== null) {
      setFileChangeDiff({
        transactionId: fileChangeTransactionId,
        pages: [inlineFileChangeDiff],
        pageIndex: 0,
        error: '',
        loading: false
      })
      return
    }

    let cancelled = false
    setFileChangeDiff({
      transactionId: fileChangeTransactionId,
      pages: [],
      pageIndex: 0,
      error: '',
      loading: true
    })
    void (async () => {
      // Keep non-FileChange approval cards independent of the Agent Host bridge. The staged Diff
      // reader is loaded only after an exact transaction requires private paginated review.
      const { getAgentFileChangeDiff } = await import('../../agent/agentClient')
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
          FILE_CHANGE_DIFF_PAGE_CHARS,
          observerRootConversationId
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
          pages: chunks,
          pageIndex: 0,
          error: '',
          loading: false
        })
      }
    })().catch(() => {
      if (!cancelled) {
        setFileChangeDiff({
          transactionId: fileChangeTransactionId,
          pages: [],
          pageIndex: 0,
          error: fileChangePreviewError,
          loading: false
        })
      }
    })
    return () => {
      cancelled = true
    }
  }, [
    fileChangePreviewError,
    fileChangeTransactionId,
    inlineFileChangeDiff,
    observerRootConversationId
  ])

  const fileChangeDiffCurrent =
    action.type === 'file_change' &&
    fileChangeDiff.transactionId === action.fileChange.transactionId
  const fileChangeDiffReady =
    action.type !== 'file_change' ||
    (fileChangeDiffCurrent && !fileChangeDiff.loading && !fileChangeDiff.error)
  const fileChangeDiffPage = fileChangeDiff.pages[fileChangeDiff.pageIndex] ?? ''

  const approve = (approvalScope: AgentApprovalScope) => {
    if (isSubmitting || !fileChangeDiffReady) return
    setIsSubmitting(true)
    resetApprovalSubmissionOnFailure(onApprove?.(messageId, action, approvalScope), () =>
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
            <div className="agent-approval-dialog__file-change-diff">
              <pre>{fileChangeDiffPage}</pre>
              {fileChangeDiff.pages.length > 1 ? (
                <nav
                  aria-label={t('agent.fileChange.togglePreview')}
                  className="agent-approval-dialog__file-change-pagination"
                >
                  <button
                    aria-label={t('files.pdf.previousPage')}
                    disabled={fileChangeDiff.pageIndex === 0}
                    onClick={() =>
                      setFileChangeDiff((current) => ({
                        ...current,
                        pageIndex: Math.max(0, current.pageIndex - 1)
                      }))
                    }
                    type="button"
                  >
                    ←
                  </button>
                  <span aria-live="polite">
                    {fileChangeDiff.pageIndex + 1} / {fileChangeDiff.pages.length}
                  </span>
                  <button
                    aria-label={t('files.pdf.nextPage')}
                    disabled={fileChangeDiff.pageIndex + 1 >= fileChangeDiff.pages.length}
                    onClick={() =>
                      setFileChangeDiff((current) => ({
                        ...current,
                        pageIndex: Math.min(current.pages.length - 1, current.pageIndex + 1)
                      }))
                    }
                    type="button"
                  >
                    →
                  </button>
                </nav>
              ) : null}
            </div>
          )
        ) : undefined
      }
      isSubmitting={isSubmitting}
      onApprove={() => approve('singleAction')}
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
                  {t('agent.approval.dialog.approveFileChangeRemember')}
                </span>
              ),
              disabled: !fileChangeDiffReady,
              onSelect: () => approve('remainingApplyPatchInRun')
            }
          : undefined
      }
      request={request || getApprovalFallbackTitle(action, t)}
    />
  )
}

export function AgentApprovalDialog({
  allowRunScopedApproval = true,
  observerRootConversationId,
  target,
  mcpInvocationState,
  onApprove,
  onCancel,
  onReject
}: AgentApprovalDialogProps) {
  const onApproveSingleAction = onApprove
    ? (messageId: string, action: AgentProposedAction) =>
        onApprove(messageId, action, 'singleAction')
    : undefined

  if (target.action.type === 'mcp_tool_call') {
    return (
      <McpToolApprovalCard
        action={target.action}
        invocationState={mcpInvocationState}
        messageId={target.messageId}
        onApprove={onApproveSingleAction}
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
        onApprove={onApproveSingleAction}
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
        onApprove={onApproveSingleAction}
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
        onApprove={onApproveSingleAction}
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
        onApprove={onApproveSingleAction}
        onReject={onReject}
      />
    )
  }

  return (
    <StandardAgentApprovalDialog
      action={target.action}
      allowRunScopedApproval={allowRunScopedApproval}
      messageId={target.messageId}
      observerRootConversationId={observerRootConversationId}
      onApprove={onApprove}
      onReject={onReject}
    />
  )
}
