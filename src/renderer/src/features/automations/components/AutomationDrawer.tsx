import { Maximize } from 'lucide-react'
import { useEffect, useRef } from 'react'
import type { AutomationRun, AutomationTask } from '@mycopilot/protocol'
import type { ModelConfig } from '../../../config/modelConfig'
import type { AppProject } from '../../../config/projectConfig'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatConversation } from '../../chat/chatTypes'
import type { AutomationDraft } from '../automationTypes'
import { healthMessage } from '../automationPresentation'
import { AutomationTaskForm } from './AutomationTaskForm'
import { AutomationRunHistory } from './AutomationRunHistory'

export type AutomationDrawerView =
  { mode: 'create'; draft: AutomationDraft } | { mode: 'task'; taskId: string }

interface AutomationDrawerProps {
  conversations: readonly ChatConversation[]
  detailError?: Error | null
  detailLoading?: boolean
  draft: AutomationDraft
  focusRunId?: string | null
  historyError?: Error | null
  historyHasMore?: boolean
  historyLoading?: boolean
  historyLoadingMore?: boolean
  maximizeDisabled?: boolean
  mode: 'create' | 'task'
  models: readonly ModelConfig[]
  mutationPending?: boolean
  maximized: boolean
  onAcknowledgeAttention: (attentionId: string) => void
  onClose: () => void
  onDirtyChange: (dirty: boolean) => void
  onHistoryLoadMore: () => void
  onHistoryRetry: () => void
  onOpenConversation: (conversationId: string, messageId?: string | null) => void
  onOpenPermissionSettings?: () => void
  onRetryDetail: () => void
  onSubmittingChange: (submitting: boolean) => void
  onSubmit: (draft: AutomationDraft) => Promise<void>
  onToggleMaximized: () => void
  permissionModeAvailability: { custom: boolean; full: boolean }
  projects: readonly AppProject[]
  runs: readonly AutomationRun[]
  task?: AutomationTask | null
}

export function AutomationDrawer({
  conversations,
  detailError,
  detailLoading = false,
  draft,
  focusRunId,
  historyError,
  historyHasMore = false,
  historyLoading = false,
  historyLoadingMore = false,
  maximizeDisabled = false,
  mode,
  models,
  mutationPending = false,
  maximized,
  onAcknowledgeAttention,
  onClose,
  onDirtyChange,
  onHistoryLoadMore,
  onHistoryRetry,
  onOpenConversation,
  onOpenPermissionSettings,
  onRetryDetail,
  onSubmittingChange,
  onSubmit,
  onToggleMaximized,
  permissionModeAvailability,
  projects,
  runs,
  task
}: AutomationDrawerProps) {
  const { t } = useFrontendConfig()
  const headingRef = useRef<HTMLHeadingElement>(null)
  const existingChatDestination =
    task?.destination.kind === 'existing_chat' ? task.destination : null
  const hasOpenableRun = runs.some((run) => Boolean(run.conversationId))
  const maximizeLabel = maximized ? t('automation.restoreDrawer') : t('automation.maximizeDrawer')
  const collapseLabel = t('automation.collapseDrawer')

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => headingRef.current?.focus())
    return () => window.cancelAnimationFrame(frame)
  }, [mode, task?.automationId])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      if (
        document.querySelector(
          '[role="dialog"][aria-modal="true"], .automation-select[data-open], .automation-multiselect[data-open], .automation-chat-picker[data-open]'
        )
      ) {
        return
      }
      if (mutationPending) return
      onClose()
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [mutationPending, onClose])

  return (
    <aside
      className="automation-drawer"
      aria-label={
        mode === 'create' ? t('automation.newTask') : (task?.title ?? t('automation.details'))
      }
      data-mode={mode}
    >
      <header className="automation-drawer__header" data-drag-region>
        <div className="automation-drawer__heading">
          <h2 ref={headingRef} tabIndex={-1}>
            {mode === 'create' ? t('automation.newTask') : (task?.title ?? t('automation.details'))}
          </h2>
        </div>
        <div className="automation-drawer__header-actions" data-no-drag-region>
          <button
            type="button"
            className="automation-icon-button"
            aria-label={collapseLabel}
            disabled={mutationPending}
            onClick={onClose}
            title={collapseLabel}
          >
            <span
              aria-hidden="true"
              className="panel-toggle__icon"
              data-open="true"
              data-side="right"
            />
          </button>
          <button
            type="button"
            className="automation-icon-button"
            aria-label={maximizeLabel}
            aria-pressed={maximized}
            disabled={maximizeDisabled}
            onClick={onToggleMaximized}
            title={maximizeLabel}
          >
            {maximized ? <RestoreFromMaximizedIcon /> : <Maximize aria-hidden="true" />}
          </button>
        </div>
      </header>

      {mode === 'task' && detailLoading && !task ? (
        <div className="automation-drawer__state" role="status">
          <span className="mc-processing-spinner" aria-hidden="true" />
          {t('automation.loading')}
        </div>
      ) : mode === 'task' && detailError && !task ? (
        <div className="automation-drawer__state" role="alert">
          <p>{t('automation.detailLoadFailed')}</p>
          <button type="button" onClick={onRetryDetail}>
            {t('automation.retry')}
          </button>
        </div>
      ) : (
        <div className="automation-drawer__content">
          {task?.health.state === 'blocked' && (
            <div className="automation-drawer__blocked" role="alert">
              <strong>{t('automation.statusBlocked')}</strong>
              <p>{healthMessage(t, task.health)}</p>
              <span>{t('automation.chooseReplacement')}</span>
              {task.attention?.readAt === null && (
                <button
                  type="button"
                  onClick={() => onAcknowledgeAttention(task.attention!.attentionId)}
                >
                  {t('automation.acknowledgeAttention')}
                </button>
              )}
            </div>
          )}
          <AutomationTaskForm
            key={mode === 'create' ? 'create' : (task?.automationId ?? 'task')}
            conversations={conversations}
            disabled={mutationPending}
            initialDraft={draft}
            mode={mode === 'create' ? 'create' : 'edit'}
            models={models}
            onCancel={onClose}
            onDirtyChange={onDirtyChange}
            onOpenPermissionSettings={onOpenPermissionSettings}
            onSubmittingChange={onSubmittingChange}
            onSubmit={onSubmit}
            permissionModeAvailability={permissionModeAvailability}
            projects={projects}
            task={task}
          />
          {mode === 'task' && task && (
            <>
              <AutomationRunHistory
                error={historyError}
                focusRunId={focusRunId}
                hasMore={historyHasMore}
                loading={historyLoading}
                loadingMore={historyLoadingMore}
                onAcknowledge={onAcknowledgeAttention}
                onLoadMore={onHistoryLoadMore}
                onOpenConversation={onOpenConversation}
                onRetry={onHistoryRetry}
                runs={runs}
              />
              {existingChatDestination && !historyLoading && !hasOpenableRun && (
                <div className="automation-drawer__open-chat">
                  <button
                    type="button"
                    className="automation-button automation-button--secondary"
                    onClick={() => onOpenConversation(existingChatDestination.conversationId, null)}
                  >
                    {t('automation.openChat')}
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      )}
    </aside>
  )
}

function RestoreFromMaximizedIcon() {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M10 4v6H4" />
      <path d="M14 4v6h6" />
      <path d="M10 20v-6H4" />
      <path d="M14 20v-6h6" />
    </svg>
  )
}
