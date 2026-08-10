import { FoldVertical, RotateCcw } from 'lucide-react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { Tooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { formatTranslation } from '../../../config/translationFormat'
import type { AgentProviderTransitionOperation } from '@mycopilot/protocol'
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
  onRetry,
  operation
}: {
  onRetry?: () => void | Promise<void>
  operation: AgentProviderTransitionOperation
}) {
  const { t } = useFrontendConfig()
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

  const button = (
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

  return (
    <div
      className="conversation-continuation-divider conversation-model-transition"
      data-status={operation.status}
      data-testid="model-transition-divider"
      role="status"
    >
      <span aria-hidden="true" />
      {tooltip ? (
        <Tooltip
          anchorClassName="conversation-model-transition__tooltip-anchor"
          content={tooltip}
          describeTrigger
        >
          {button}
        </Tooltip>
      ) : (
        button
      )}
      <span aria-hidden="true" />
    </div>
  )
}
