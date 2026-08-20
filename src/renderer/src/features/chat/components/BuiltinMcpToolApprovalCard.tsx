import { useEffect, useRef, useState } from 'react'
import type { AgentBuiltinMcpToolRiskKind, AgentProposedAction } from '@mycopilot/protocol'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import { getBuiltinCapabilityDisplayName } from '../../mcp/builtinCapabilityPresentation'
import { toSafeMcpDisplayText } from '../../mcp/mcpSafeDisplay'
import { ApprovalDialogShell } from './ApprovalDialogShell'

type BuiltinMcpToolApprovalAction = Extract<
  AgentProposedAction,
  { type: 'builtin_mcp_tool_approval' }
>

interface BuiltinMcpToolApprovalCardProps {
  action: BuiltinMcpToolApprovalAction
  messageId: string
  onApprove?: (messageId: string, action: AgentProposedAction) => void
  onCancel?: (messageId: string, action: AgentProposedAction) => void
  onReject?: (messageId: string, action: AgentProposedAction, message?: string) => void
}

const toolKeys: Readonly<Record<string, TranslationKey>> = {
  browser_cookie_clear: 'agent.builtinMcpApproval.tool.browser_cookie_clear',
  browser_cookie_delete: 'agent.builtinMcpApproval.tool.browser_cookie_delete',
  browser_cookie_get: 'agent.builtinMcpApproval.tool.browser_cookie_get',
  browser_cookie_list: 'agent.builtinMcpApproval.tool.browser_cookie_list',
  browser_cookie_set: 'agent.builtinMcpApproval.tool.browser_cookie_set',
  browser_drop: 'agent.builtinMcpApproval.tool.browser_drop',
  browser_evaluate: 'agent.builtinMcpApproval.tool.browser_evaluate',
  browser_file_upload: 'agent.builtinMcpApproval.tool.browser_file_upload',
  browser_localstorage_clear: 'agent.builtinMcpApproval.tool.browser_localstorage_clear',
  browser_localstorage_delete: 'agent.builtinMcpApproval.tool.browser_localstorage_delete',
  browser_localstorage_get: 'agent.builtinMcpApproval.tool.browser_localstorage_get',
  browser_localstorage_list: 'agent.builtinMcpApproval.tool.browser_localstorage_list',
  browser_localstorage_set: 'agent.builtinMcpApproval.tool.browser_localstorage_set',
  browser_network_request: 'agent.builtinMcpApproval.tool.browser_network_request',
  browser_sessionstorage_clear: 'agent.builtinMcpApproval.tool.browser_sessionstorage_clear',
  browser_sessionstorage_delete: 'agent.builtinMcpApproval.tool.browser_sessionstorage_delete',
  browser_sessionstorage_get: 'agent.builtinMcpApproval.tool.browser_sessionstorage_get',
  browser_sessionstorage_list: 'agent.builtinMcpApproval.tool.browser_sessionstorage_list',
  browser_sessionstorage_set: 'agent.builtinMcpApproval.tool.browser_sessionstorage_set',
  browser_set_storage_state: 'agent.builtinMcpApproval.tool.browser_set_storage_state',
  browser_storage_state: 'agent.builtinMcpApproval.tool.browser_storage_state'
}

const riskKeys: Readonly<Record<AgentBuiltinMcpToolRiskKind, TranslationKey>> = {
  file_read: 'agent.builtinMcpApproval.risk.file_read',
  file_write: 'agent.builtinMcpApproval.risk.file_write',
  file_upload: 'agent.builtinMcpApproval.risk.file_upload',
  file_download: 'agent.builtinMcpApproval.risk.file_download',
  cookie_read: 'agent.builtinMcpApproval.risk.cookie_read',
  cookie_write: 'agent.builtinMcpApproval.risk.cookie_write',
  local_storage_read: 'agent.builtinMcpApproval.risk.local_storage_read',
  local_storage_write: 'agent.builtinMcpApproval.risk.local_storage_write',
  session_storage_read: 'agent.builtinMcpApproval.risk.session_storage_read',
  session_storage_write: 'agent.builtinMcpApproval.risk.session_storage_write',
  storage_state_import: 'agent.builtinMcpApproval.risk.storage_state_import',
  storage_state_export: 'agent.builtinMcpApproval.risk.storage_state_export',
  network_sensitive_read: 'agent.builtinMcpApproval.risk.network_sensitive_read',
  page_script_execution: 'agent.builtinMcpApproval.risk.page_script_execution',
  unsafe_code_execution: 'agent.builtinMcpApproval.risk.unsafe_code_execution'
}

const resourceScopeKeys: Readonly<Record<string, TranslationKey>> = {
  managed_browser_profile: 'agent.builtinMcpApproval.resource.managedBrowserProfile',
  managed_surface: 'agent.builtinMcpApproval.resource.managedSurface'
}

function currentUnixSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

/** Renders only Core's value-free allowlist projection; all values remain React text nodes. */
export function BuiltinMcpToolApprovalCard({
  action,
  messageId,
  onApprove,
  onCancel,
  onReject
}: BuiltinMcpToolApprovalCardProps) {
  const { language, t } = useFrontendConfig()
  const submittingRef = useRef(false)
  const { approval } = action
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [rejectMessage, setRejectMessage] = useState('')
  const [expired, setExpired] = useState(() => currentUnixSeconds() >= approval.expiresAt)

  useEffect(() => {
    submittingRef.current = false
    setIsSubmitting(false)
    setRejectMessage('')
    const remainingMs = approval.expiresAt * 1000 - Date.now()
    if (remainingMs <= 0) {
      setExpired(true)
      return
    }
    setExpired(false)
    const timeoutId = window.setTimeout(() => setExpired(true), remainingMs)
    return () => window.clearTimeout(timeoutId)
  }, [approval.identity.actionId, approval.expiresAt, messageId])

  const pending = approval.approvalStatus === 'required'
  const canApprove = pending && !expired && Boolean(onApprove) && !isSubmitting
  const canReject = pending && Boolean(onReject) && !isSubmitting
  const canCancel = pending && Boolean(onCancel) && !isSubmitting
  const submitOnce = (operation: (() => void) | undefined) => {
    if (!operation || submittingRef.current) return
    submittingRef.current = true
    setIsSubmitting(true)
    operation()
  }

  const capability = getBuiltinCapabilityDisplayName(approval.identity.capabilityId, t)
  const toolKey = toolKeys[approval.identity.toolId]
  const toolName = toolKey ? t(toolKey) : t('agent.builtinMcpApproval.tool.unknown')
  const callReason = toSafeMcpDisplayText(approval.callReason, 4096)
  const scopeKey =
    resourceScopeKeys[approval.resourceSummary.scope] ??
    riskKeys[approval.resourceSummary.scope as AgentBuiltinMcpToolRiskKind]
  const resourceName = scopeKey ? t(scopeKey) : t('agent.builtinMcpApproval.resource.generic')
  const origin = approval.resourceSummary.origin
    ? toSafeMcpDisplayText(approval.resourceSummary.origin, 2048)
    : null
  const fileNames = approval.resourceSummary.fileBasenames.map((name) =>
    toSafeMcpDisplayText(name, 256)
  )
  const risks = approval.riskKinds.map((risk) => t(riskKeys[risk])).join(' · ')
  const createdAt = new Date(approval.createdAt * 1000)
  const expiresAt = new Date(approval.expiresAt * 1000)
  const formatTime = (value: number) =>
    new Intl.DateTimeFormat(language, { dateStyle: 'short', timeStyle: 'medium' }).format(
      new Date(value * 1000)
    )

  return (
    <ApprovalDialogShell
      approvalKind="standard"
      approveDisabled={!canApprove}
      approveLabel={t('agent.approval.dialog.approve')}
      ariaBusy={isSubmitting}
      details={
        <dl className="agent-builtin-mcp-tool-approval__details">
          <div>
            <dt>{t('agent.builtinMcpApproval.reason')}</dt>
            <dd>{callReason}</dd>
          </div>
          <div>
            <dt>{t('agent.builtinMcpApproval.operation')}</dt>
            <dd>{toolName}</dd>
          </div>
          <div>
            <dt>{t('agent.builtinMcpApproval.resource')}</dt>
            <dd>{resourceName}</dd>
          </div>
          {origin ? (
            <div>
              <dt>{t('agent.builtinMcpApproval.origin')}</dt>
              <dd>{origin}</dd>
            </div>
          ) : null}
          {fileNames.length > 0 ? (
            <div>
              <dt>{t('agent.builtinMcpApproval.files')}</dt>
              <dd>{fileNames.join(' · ')}</dd>
            </div>
          ) : null}
          <div>
            <dt>{t('agent.builtinMcpApproval.risks')}</dt>
            <dd>{risks}</dd>
          </div>
          <div>
            <dt>{t('agent.builtinMcpApproval.createdAt')}</dt>
            <dd>
              <time dateTime={createdAt.toISOString()}>{formatTime(approval.createdAt)}</time>
            </dd>
          </div>
          <div>
            <dt>{t('agent.builtinMcpApproval.expiresAt')}</dt>
            <dd>
              <time dateTime={expiresAt.toISOString()}>{formatTime(approval.expiresAt)}</time>
            </dd>
          </div>
        </dl>
      }
      isSubmitting={isSubmitting}
      onApprove={() => submitOnce(() => onApprove?.(messageId, action))}
      onCancel={canCancel ? () => submitOnce(() => onCancel?.(messageId, action)) : undefined}
      onReject={(message) => {
        if (!canReject) return
        const guidance = message.trim()
        submitOnce(() => onReject?.(messageId, action, guidance || undefined))
      }}
      onRejectMessageChange={setRejectMessage}
      policyHint={
        expired
          ? t('agent.builtinMcpApproval.expired')
          : t('agent.builtinMcpApproval.exactScopeHint')
      }
      policyTone={expired ? 'danger' : 'default'}
      rejectDisabled={!canReject}
      rejectLabel={t('agent.approval.dialog.reject')}
      rejectMessage={rejectMessage}
      rejectPlaceholder={t('agent.approval.dialog.rejectPlaceholder')}
      request={formatTranslation(t, 'agent.builtinMcpApproval.title', {
        capability,
        tool: toolName
      })}
    />
  )
}
