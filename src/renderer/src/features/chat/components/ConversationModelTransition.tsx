import { useRef, useState } from 'react'
import { FoldVertical, LoaderCircle, RotateCcw, Split } from 'lucide-react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type {
  AgentProviderTransitionOperation,
  AgentManualContextCompactionOperation
} from '@mycopilot/protocol'
import type { ModelTransitionConfirmation } from '../modelTransitionUiState'

export function ModelTransitionConfirmationDialog({
  onCancel,
  onConfirm,
  preflight
}: {
  onCancel: () => void
  onConfirm: () => void | Promise<void>
  preflight: ModelTransitionConfirmation
}) {
  const { t } = useFrontendConfig()
  const isProviderChange = preflight.reason === 'api_provider_changed'

  return (
    <ConfirmationDialog
      cancelLabel={t('chat.modelTransition.cancel')}
      confirmLabel={t('chat.modelTransition.confirm')}
      confirmVariant="primary"
      description={
        isProviderChange
          ? t('chat.modelTransition.providerDescription')
          : t('chat.modelTransition.protocolDescription')
      }
      onCancel={onCancel}
      onConfirm={onConfirm}
      title={
        isProviderChange
          ? t('chat.modelTransition.providerTitle')
          : t('chat.modelTransition.protocolTitle')
      }
    />
  )
}

export function ConversationModelTransitionDivider({
  onContinueInNewTask,
  onRetry,
  operation
}: {
  onContinueInNewTask?: () => void | Promise<void>
  onRetry?: () => void | Promise<void>
  operation: AgentProviderTransitionOperation
}) {
  const { t } = useFrontendConfig()
  const [isContinuing, setIsContinuing] = useState(false)
  const isContinuingRef = useRef(false)
  const content =
    operation.status === 'running'
      ? t('chat.modelTransition.running')
      : operation.status === 'completed'
        ? t('chat.modelTransition.succeeded')
        : t('chat.modelTransition.failed')

  const icon =
    operation.status === 'failed' ? (
      <RotateCcw aria-hidden="true" />
    ) : (
      <FoldVertical aria-hidden="true" />
    )
  const tooltip =
    operation.sourceModelDisplayName && operation.targetModelDisplayName
      ? formatTranslation(t, 'chat.modelTransition.modelChangeTooltip', {
          source: operation.sourceModelDisplayName,
          target: operation.targetModelDisplayName
        })
      : null

  const statusButton = (
    <button
      aria-label={content}
      disabled={operation.status !== 'failed' || !onRetry}
      onClick={() => {
        if (operation.status === 'failed' && onRetry) void onRetry()
      }}
      type="button"
    >
      {icon}
      <span className={operation.status === 'running' ? 'agent-running-text' : undefined}>
        {content}
      </span>
    </button>
  )
  const status = tooltip ? (
    <Tooltip
      anchorClassName="conversation-model-transition__tooltip-anchor"
      content={tooltip}
      describeTrigger
    >
      {statusButton}
    </Tooltip>
  ) : (
    statusButton
  )

  return (
    <div
      className="conversation-continuation-divider conversation-model-transition"
      data-status={operation.status}
      data-testid="model-transition-divider"
      role="status"
    >
      <span aria-hidden="true" />
      <div className="conversation-model-transition__content">
        {status}
        {operation.status === 'completed' && operation.summaryId && onContinueInNewTask && (
          <Tooltip content={t('chat.continueInNewTask')}>
            <button
              aria-label={t('chat.continueInNewTask')}
              className="conversation-model-transition__fork"
              disabled={isContinuing}
              onClick={() => {
                if (isContinuingRef.current) return
                isContinuingRef.current = true
                setIsContinuing(true)
                void Promise.resolve(onContinueInNewTask()).finally(() => {
                  isContinuingRef.current = false
                  setIsContinuing(false)
                })
              }}
              title={t('chat.continueInNewTask')}
              type="button"
            >
              {isContinuing ? (
                <LoaderCircle aria-hidden="true" className="chat-message__action-spinner" />
              ) : (
                <Split aria-hidden="true" />
              )}
            </button>
          </Tooltip>
        )}
      </div>
      <span aria-hidden="true" />
    </div>
  )
}

/** Same timeline divider and running animation as provider compaction. No chat message is created. */
export function ConversationManualCompactionDivider({
  operation
}: {
  operation: AgentManualContextCompactionOperation
}) {
  const { t } = useFrontendConfig()
  const content = t(`chat.manualCompaction.${operation.status}`)
  return (
    <div
      className="conversation-continuation-divider conversation-model-transition"
      data-status={operation.status}
      data-testid="manual-compaction-divider"
      role="status"
    >
      <span aria-hidden="true" />
      <div className="conversation-model-transition__content">
        <button type="button" disabled aria-label={content} title={operation.error}>
          <FoldVertical aria-hidden="true" />
          <span className={operation.status === 'running' ? 'agent-running-text' : undefined}>
            {content}
          </span>
        </button>
      </div>
      <span aria-hidden="true" />
    </div>
  )
}
