import { MoreHorizontal, PauseCircle, Play, Trash2, X } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { AutomationRun, AutomationTask } from '@mycopilot/protocol'
import type { ModelConfig } from '../../../config/modelConfig'
import type { AppProject } from '../../../config/projectConfig'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { ChatConversation } from '../../chat/chatTypes'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import type { AutomationDraft } from '../automationTypes'
import { healthMessage, isTaskRunActive } from '../automationPresentation'
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
  mode: 'create' | 'task'
  models: readonly ModelConfig[]
  mutationPending?: boolean
  onAcknowledgeAttention: (attentionId: string) => void
  onClose: () => void
  onDelete: (task: AutomationTask) => void
  onDirtyChange: (dirty: boolean) => void
  onHistoryLoadMore: () => void
  onHistoryRetry: () => void
  onOpenConversation: (conversationId: string, messageId?: string | null) => void
  onOpenPermissionSettings?: () => void
  onRetryDetail: () => void
  onRunNow: (task: AutomationTask) => void
  onSetEnabled: (task: AutomationTask, enabled: boolean) => void
  onSubmittingChange: (submitting: boolean) => void
  onSubmit: (draft: AutomationDraft) => Promise<void>
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
  mode,
  models,
  mutationPending = false,
  onAcknowledgeAttention,
  onClose,
  onDelete,
  onDirtyChange,
  onHistoryLoadMore,
  onHistoryRetry,
  onOpenConversation,
  onOpenPermissionSettings,
  onRetryDetail,
  onRunNow,
  onSetEnabled,
  onSubmittingChange,
  onSubmit,
  permissionModeAvailability,
  projects,
  runs,
  task
}: AutomationDrawerProps) {
  const { t } = useFrontendConfig()
  const [menuOpen, setMenuOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const headingRef = useRef<HTMLHeadingElement>(null)
  const closeMenu = useCallback(() => setMenuOpen(false), [])
  useDismissOnOutsidePointer(menuRef, menuOpen, closeMenu)
  const activeRun = task ? isTaskRunActive(task) : false
  const existingChatDestination =
    task?.destination.kind === 'existing_chat' ? task.destination : null

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => headingRef.current?.focus())
    return () => window.cancelAnimationFrame(frame)
  }, [mode, task?.automationId])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      if (menuOpen) {
        closeMenu()
        return
      }
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
  }, [closeMenu, menuOpen, mutationPending, onClose])

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
          {mode === 'task' && task && (
            <span
              className="automation-drawer__status"
              data-status={task.health.state === 'blocked' ? 'blocked' : task.status}
            >
              {task.health.state === 'blocked'
                ? t('automation.statusBlocked')
                : task.status === 'active'
                  ? t('automation.statusActive')
                  : t('automation.statusPaused')}
            </span>
          )}
          <h2 ref={headingRef} tabIndex={-1}>
            {mode === 'create' ? t('automation.newTask') : (task?.title ?? t('automation.details'))}
          </h2>
        </div>
        <div className="automation-drawer__header-actions" data-no-drag-region>
          {mode === 'task' && task && (
            <>
              <div className="automation-drawer__menu-root" ref={menuRef}>
                <button
                  type="button"
                  className="automation-icon-button"
                  aria-label={t('automation.moreActions')}
                  aria-expanded={menuOpen}
                  aria-haspopup="menu"
                  disabled={mutationPending}
                  onClick={() => setMenuOpen((open) => !open)}
                >
                  <MoreHorizontal aria-hidden="true" />
                </button>
                {menuOpen && (
                  <div className="automation-task-menu automation-task-menu--drawer" role="menu">
                    <button
                      type="button"
                      role="menuitem"
                      onClick={() => {
                        closeMenu()
                        onSetEnabled(task, task.status === 'paused')
                      }}
                    >
                      {task.status === 'paused' ? (
                        <Play aria-hidden="true" />
                      ) : (
                        <PauseCircle aria-hidden="true" />
                      )}
                      <span>
                        {task.status === 'paused' ? t('automation.resume') : t('automation.pause')}
                      </span>
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      className="automation-task-menu__danger"
                      onClick={() => {
                        closeMenu()
                        onDelete(task)
                      }}
                    >
                      <Trash2 aria-hidden="true" />
                      <span>{t('automation.delete')}</span>
                    </button>
                  </div>
                )}
              </div>
              <button
                type="button"
                className="automation-icon-button"
                aria-label={activeRun ? t('automation.runningAction') : t('automation.runNow')}
                disabled={activeRun || mutationPending || task.health.state === 'blocked'}
                onClick={() => onRunNow(task)}
              >
                <Play aria-hidden="true" />
              </button>
            </>
          )}
          <button
            type="button"
            className="automation-icon-button"
            aria-label={t('automation.close')}
            disabled={mutationPending}
            onClick={onClose}
          >
            <X aria-hidden="true" />
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
              {existingChatDestination && (
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
