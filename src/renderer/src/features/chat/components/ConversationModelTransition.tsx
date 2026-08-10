import { Check, LoaderCircle, RotateCcw } from 'lucide-react'
import { ConfirmationDialog } from '../../../components/dialog/ConfirmationDialog'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
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
    operation.status === 'running' ? (
      <LoaderCircle aria-hidden="true" className="conversation-model-transition__spinner" />
    ) : operation.status === 'completed' ? (
      <Check aria-hidden="true" />
    ) : (
      <RotateCcw aria-hidden="true" />
    )

  return (
    <div
      className="conversation-continuation-divider conversation-model-transition"
      data-status={operation.status}
      data-testid="model-transition-divider"
      role="status"
    >
      <span aria-hidden="true" />
      <button
        aria-label={content}
        disabled={operation.status !== 'failed' || !onRetry}
        onClick={() => {
          if (operation.status === 'failed' && onRetry) void onRetry()
        }}
        type="button"
      >
        {icon}
        <span>{content}</span>
      </button>
      <span aria-hidden="true" />
    </div>
  )
}
