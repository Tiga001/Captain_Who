import { useEffect, useId, useRef, useState } from 'react'
import type {
  AgentMcpServerScope,
  AgentMcpToolApproval,
  AgentMcpToolInvocationState,
  AgentMcpToolRisk,
  AgentProposedAction
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { TranslationKey } from '../../../config/frontendTranslations'
import { toSafeMcpDisplayText } from '../../mcp/mcpSafeDisplay'

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

const riskTranslationKeys = {
  unknown: 'agent.mcp.approval.risk.unknown',
  read_only_claimed: 'agent.mcp.approval.risk.readOnlyClaimed',
  side_effects_possible: 'agent.mcp.approval.risk.sideEffectsPossible',
  destructive_claimed: 'agent.mcp.approval.risk.destructiveClaimed',
  open_world_claimed: 'agent.mcp.approval.risk.openWorldClaimed'
} as const satisfies Record<AgentMcpToolRisk, TranslationKey>

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
} as const satisfies Record<AgentMcpToolInvocationState, TranslationKey>

function getScopeTranslationKey(scope: AgentMcpServerScope): TranslationKey {
  switch (scope.type) {
    case 'builtin':
      return 'agent.mcp.approval.scope.builtin'
    case 'user':
      return 'agent.mcp.approval.scope.user'
    case 'project':
      return 'agent.mcp.approval.scope.project'
    case 'plugin':
      return 'agent.mcp.approval.scope.plugin'
    case 'managed':
      return 'agent.mcp.approval.scope.managed'
  }
}

function getScopeDetail(scope: AgentMcpServerScope): string | undefined {
  switch (scope.type) {
    case 'project':
      return scope.projectId
    case 'plugin':
      return scope.pluginId
    default:
      return undefined
  }
}

function isElevatedRisk(risk: AgentMcpToolRisk): boolean {
  return (
    risk === 'side_effects_possible' ||
    risk === 'destructive_claimed' ||
    risk === 'open_world_claimed'
  )
}

function formatApprovalTime(timestamp: number, language: string): string {
  const date = new Date(timestamp)
  if (Number.isNaN(date.getTime())) return '—'
  return new Intl.DateTimeFormat(language, {
    dateStyle: 'medium',
    timeStyle: 'medium'
  }).format(date)
}

function getBlockingState(
  approval: AgentMcpToolApproval,
  invocationState: AgentMcpToolInvocationState | undefined,
  expired: boolean
): AgentMcpToolInvocationState | undefined {
  if (expired) return 'expired'
  if (approval.approvalMode === 'deny') return 'policy_denied'
  if (invocationState && !APPROVABLE_INVOCATION_STATES.has(invocationState)) {
    return invocationState
  }
  return undefined
}

export function McpToolApprovalCard({
  action,
  invocationState,
  messageId,
  onApprove,
  onCancel,
  onReject
}: McpToolApprovalCardProps) {
  const { language, t } = useFrontendConfig()
  const titleId = useId()
  const rejectionInputId = useId()
  const submittingRef = useRef(false)
  const { approval } = action
  const [expired, setExpired] = useState(() => Date.now() >= approval.expiresAt)
  const [rejectionMessage, setRejectionMessage] = useState('')
  const [isSubmitting, setIsSubmitting] = useState(false)

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
  const scopeDetail = getScopeDetail(approval.summary.scope)

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

  const reject = () => {
    if (!canReject) return
    submitOnce(() =>
      onReject?.(messageId, action, rejectionMessage.length > 0 ? rejectionMessage : undefined)
    )
  }

  const cancel = () => {
    if (!canCancel) return
    submitOnce(() => onCancel?.(messageId, action))
  }

  return (
    <section
      aria-busy={isSubmitting}
      aria-labelledby={titleId}
      className="mcp-tool-approval-card"
      role="dialog"
    >
      <header className="mcp-tool-approval-card__header">
        <div>
          <span className="mcp-tool-approval-card__external">
            {t('agent.mcp.approval.externalBadge')}
          </span>
          <h2 id={titleId}>{t('agent.mcp.approval.title')}</h2>
        </div>
        <span
          className="mcp-tool-approval-card__state"
          data-terminal={blockingState ? 'true' : undefined}
          role="status"
        >
          {t(stateTranslationKeys[blockingState ?? currentState])}
        </span>
      </header>

      <p
        className="mcp-tool-approval-card__risk"
        data-elevated={isElevatedRisk(approval.summary.risk) ? 'true' : undefined}
        role={isElevatedRisk(approval.summary.risk) ? 'alert' : 'status'}
      >
        <strong>{t('agent.mcp.approval.riskLabel')}</strong>
        <span>{t(riskTranslationKeys[approval.summary.risk])}</span>
      </p>

      <dl className="mcp-tool-approval-card__identity">
        <div>
          <dt>{t('agent.mcp.approval.serverLabel')}</dt>
          <dd>{toSafeMcpDisplayText(approval.summary.serverDisplayName)}</dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.serverIdLabel')}</dt>
          <dd>{toSafeMcpDisplayText(approval.summary.serverId)}</dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.scopeLabel')}</dt>
          <dd>
            {t(getScopeTranslationKey(approval.summary.scope))}
            {scopeDetail ? <span> · {toSafeMcpDisplayText(scopeDetail)}</span> : null}
          </dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.rawToolLabel')}</dt>
          <dd>{toSafeMcpDisplayText(approval.summary.rawToolName)}</dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.modelToolLabel')}</dt>
          <dd>{toSafeMcpDisplayText(approval.summary.modelToolName)}</dd>
        </div>
      </dl>

      <section
        aria-labelledby={`${titleId}-arguments`}
        className="mcp-tool-approval-card__arguments"
      >
        <h3 id={`${titleId}-arguments`}>{t('agent.mcp.approval.argumentsTitle')}</h3>
        <dl>
          <div>
            <dt>{t('agent.mcp.approval.encodedBytes')}</dt>
            <dd>{approval.summary.arguments.encodedBytes}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.topLevelProperties')}</dt>
            <dd>{approval.summary.arguments.topLevelPropertyCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.maxDepth')}</dt>
            <dd>{approval.summary.arguments.maxDepth}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.strings')}</dt>
            <dd>{approval.summary.arguments.stringValueCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.numbers')}</dt>
            <dd>{approval.summary.arguments.numberValueCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.booleans')}</dt>
            <dd>{approval.summary.arguments.booleanValueCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.nulls')}</dt>
            <dd>{approval.summary.arguments.nullValueCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.objects')}</dt>
            <dd>{approval.summary.arguments.objectValueCount}</dd>
          </div>
          <div>
            <dt>{t('agent.mcp.approval.arrays')}</dt>
            <dd>{approval.summary.arguments.arrayValueCount}</dd>
          </div>
        </dl>
        {approval.summary.arguments.truncated ? (
          <p className="mcp-tool-approval-card__truncated" role="status">
            {t('agent.mcp.approval.truncated')}
          </p>
        ) : null}
      </section>

      <dl className="mcp-tool-approval-card__metadata">
        <div>
          <dt>{t('agent.mcp.approval.createdAtLabel')}</dt>
          <dd>{formatApprovalTime(approval.createdAt, language)}</dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.expiresAtLabel')}</dt>
          <dd>{formatApprovalTime(approval.expiresAt, language)}</dd>
        </div>
        <div>
          <dt>{t('agent.mcp.approval.payloadLabel')}</dt>
          <dd>
            {approval.payloadPersistence === 'process_only'
              ? t('agent.mcp.approval.payload.processOnly')
              : t('agent.mcp.approval.payload.durable')}
          </dd>
        </div>
      </dl>

      {blockingState ? (
        <p className="mcp-tool-approval-card__blocked" role="alert">
          {t(stateTranslationKeys[blockingState])}
        </p>
      ) : (
        <p className="mcp-tool-approval-card__policy">{t('agent.mcp.approval.policyPrompt')}</p>
      )}

      <div className="mcp-tool-approval-card__actions">
        <button disabled={!canApprove} onClick={approve} type="button">
          {t('agent.mcp.approval.approve')}
        </button>
        <button disabled={!canCancel} onClick={cancel} type="button">
          {t('agent.mcp.approval.cancel')}
        </button>
      </div>

      <div className="mcp-tool-approval-card__reject">
        <label htmlFor={rejectionInputId}>{t('agent.mcp.approval.rejectingPlaceholder')}</label>
        <div>
          <input
            disabled={!canReject}
            id={rejectionInputId}
            onChange={(event) => setRejectionMessage(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                event.preventDefault()
                reject()
              }
            }}
            value={rejectionMessage}
          />
          <button disabled={!canReject} onClick={reject} type="button">
            {t('agent.mcp.approval.reject')}
          </button>
        </div>
      </div>
    </section>
  )
}
