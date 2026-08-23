import { Clock3, Plus, RefreshCw, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import type { AutomationTask } from '@mycopilot/protocol'
import { ConfirmationDialog } from '../../components/dialog/ConfirmationDialog'
import { useToast } from '../../components/toast/ToastContext'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import type { ModelConfig } from '../../config/modelConfig'
import type { AppProject } from '../../config/projectConfig'
import type { ChatConversation, ChatPermissionMode } from '../chat/chatTypes'
import { createDefaultAutomationSchedule, withSystemTimeZone } from './automationSchedule'
import type { AutomationDraft, AutomationFilter, AutomationMutationInput } from './automationTypes'
import { useAutomations } from './useAutomations'
import { useAutomationAttention } from './useAutomationAttention'
import { useAutomationDetail } from './useAutomationDetail'
import { useAutomationRuns } from './useAutomationRuns'
import { useAutomationLayout } from './useAutomationLayout'
import { AutomationDrawer } from './components/AutomationDrawer'
import { AutomationDrawerResizeHandle } from './components/AutomationDrawerResizeHandle'
import { AutomationTaskRow } from './components/AutomationTaskRow'

export interface ScheduledOpenRequest {
  automationId: string
  requestKey: number
  runId: string | null
}

export interface ScheduledExternalNavigationRequest {
  proceed: () => void
  requestKey: number
}

export interface ScheduledPageProps {
  conversations: readonly ChatConversation[]
  defaultModelId: string | null
  defaultPermissionMode: ChatPermissionMode
  defaultProjectId: string | null
  externalNavigationRequest?: ScheduledExternalNavigationRequest
  initialPreferredDrawerWidth?: number
  models: readonly ModelConfig[]
  onOpenConversation: (conversationId: string, messageId?: string | null) => void
  onOpenPermissionSettings?: () => void
  onPreferredDrawerWidthChange?: (width: number) => void
  openRequest?: ScheduledOpenRequest
  permissionModeAvailability: { custom: boolean; full: boolean }
  projects: readonly AppProject[]
}

type DrawerState =
  | { mode: 'closed' }
  | { mode: 'create'; draft: AutomationDraft }
  | { mode: 'task'; taskId: string; focusRunId: string | null }

interface PendingNavigation {
  key: number
  action: () => void
}

function createDraft({
  defaultModelId,
  defaultPermissionMode,
  defaultProjectId,
  models
}: Pick<
  ScheduledPageProps,
  'defaultModelId' | 'defaultPermissionMode' | 'defaultProjectId' | 'models'
>): AutomationDraft {
  const preferredModel =
    models.find((model) => model.id === defaultModelId && model.enabled) ??
    models.find((model) => model.enabled) ??
    models[0]
  return {
    title: '',
    prompt: '',
    status: 'active',
    destination: {
      kind: 'new_chat',
      projectBinding: defaultProjectId ? 'project' : 'none',
      projectId: defaultProjectId,
      modelId: preferredModel?.id ?? ''
    },
    permissionMode: defaultPermissionMode,
    permissionModeVersion: 1,
    schedule: createDefaultAutomationSchedule(),
    notificationPolicy: 'all_runs'
  }
}

function taskToDraft(task: AutomationTask): AutomationDraft {
  return {
    title: task.title,
    prompt: task.prompt,
    status: task.status,
    destination:
      task.destination.kind === 'new_chat'
        ? {
            kind: 'new_chat',
            projectBinding: task.destination.projectBinding,
            projectId: task.destination.projectId,
            modelId: task.destination.modelId
          }
        : { kind: 'existing_chat', conversationId: task.destination.conversationId },
    permissionMode: task.permissionMode,
    permissionModeVersion: task.permissionModeVersion,
    // Keep the authoritative saved zone while merely viewing a task. An actual
    // edit submission is normalized to the computer zone at the request boundary.
    schedule: task.schedule,
    notificationPolicy: task.notificationPolicy
  }
}

function draftToMutation(draft: AutomationDraft): AutomationMutationInput {
  return {
    title: draft.title,
    prompt: draft.prompt,
    destination: draft.destination,
    permissionMode: draft.permissionMode,
    permissionModeVersion: draft.permissionModeVersion,
    schedule: withSystemTimeZone(draft.schedule),
    notificationPolicy: draft.notificationPolicy
  }
}

function mutationErrorMessage(error: unknown, fallback: string, t: Translate): string {
  if (!error || typeof error !== 'object') return fallback
  const details = error as { code?: unknown }
  if (details.code === 'run_already_active') return t('automation.runAlreadyActive')
  if (details.code === 'revision_conflict') return t('automation.revisionConflict')
  if (details.code === 'not_found') return t('automation.updatedElsewhere')
  if (details.code === 'target_invalid') return t('automation.targetMissing')
  if (details.code === 'permission_disabled') return t('automation.permissionRepair')
  if (details.code === 'schedule_invalid') return t('automation.scheduleInvalid')
  return fallback
}

export function ScheduledPage({
  conversations,
  defaultModelId,
  defaultPermissionMode,
  defaultProjectId,
  externalNavigationRequest,
  initialPreferredDrawerWidth,
  models,
  onOpenConversation,
  onOpenPermissionSettings,
  onPreferredDrawerWidthChange,
  openRequest,
  permissionModeAvailability,
  projects
}: ScheduledPageProps) {
  const { showToast } = useToast()
  const { t } = useFrontendConfig()
  const [filter, setFilter] = useState<AutomationFilter>('all')
  const [query, setQuery] = useState('')
  const [drawer, setDrawer] = useState<DrawerState>({ mode: 'closed' })
  const [drawerMaximized, setDrawerMaximized] = useState(false)
  const [drawerDirty, setDrawerDirty] = useState(false)
  const [formSubmitting, setFormSubmitting] = useState(false)
  const [pendingNavigation, setPendingNavigation] = useState<PendingNavigation | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<AutomationTask | null>(null)
  const pageRef = useRef<HTMLElement>(null)
  const createButtonRef = useRef<HTMLButtonElement>(null)
  const drawerOpenerRef = useRef<HTMLElement | null>(null)
  const previousDrawerModeRef = useRef<DrawerState['mode']>('closed')
  const navigationKeyRef = useRef(0)
  const handledExternalNavigationRef = useRef<number | null>(null)
  const handledOpenRequestRef = useRef<number | null>(null)

  const automations = useAutomations({ enabled: true, filter, query, limit: 50 })
  const selectedTaskId = drawer.mode === 'task' ? drawer.taskId : null
  const detail = useAutomationDetail(selectedTaskId, drawer.mode === 'task')
  const history = useAutomationRuns(selectedTaskId, drawer.mode === 'task')
  const attention = useAutomationAttention(true)
  const selectedListTask = automations.tasks.find((task) => task.automationId === selectedTaskId)
  const selectedTask = detail.task ?? selectedListTask ?? null
  const mutationPending = selectedTaskId ? Boolean(automations.pendingById[selectedTaskId]) : false
  const drawerMutationPending = drawer.mode === 'create' ? automations.isCreating : mutationPending
  const navigationBusy = drawerMutationPending || formSubmitting
  const layout = useAutomationLayout({
    containerRef: pageRef,
    drawerOpen: drawer.mode !== 'closed',
    initialPreferredDrawerWidth,
    onPreferredDrawerWidthChange
  })
  const drawerEffectivelyMaximized = layout.drawerCoversList || drawerMaximized
  const drawerCoversList = drawer.mode !== 'closed' && drawerEffectivelyMaximized

  useEffect(() => {
    const previousMode = previousDrawerModeRef.current
    previousDrawerModeRef.current = drawer.mode
    if (previousMode === 'closed' || drawer.mode !== 'closed') return

    const opener = drawerOpenerRef.current
    drawerOpenerRef.current = null
    const frame = window.requestAnimationFrame(() => {
      const focusTarget = opener?.isConnected ? opener : createButtonRef.current
      focusTarget?.focus()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [drawer.mode])

  const requestNavigation = useCallback(
    (action: () => void) => {
      if (navigationBusy) {
        showToast(t('automation.operationInProgress'))
        return false
      }
      if (!drawerDirty) {
        action()
        return true
      }
      navigationKeyRef.current += 1
      setPendingNavigation({ key: navigationKeyRef.current, action })
      return true
    },
    [drawerDirty, navigationBusy, showToast, t]
  )

  useEffect(() => {
    if (!openRequest || handledOpenRequestRef.current === openRequest.requestKey) return
    const accepted = requestNavigation(() => {
      if (drawer.mode === 'closed') {
        drawerOpenerRef.current =
          document.activeElement instanceof HTMLElement ? document.activeElement : null
      }
      setDrawer({
        mode: 'task',
        taskId: openRequest.automationId,
        focusRunId: openRequest.runId
      })
      setDrawerMaximized(false)
      setDrawerDirty(false)
    })
    if (accepted) handledOpenRequestRef.current = openRequest.requestKey
  }, [drawer.mode, openRequest, requestNavigation])

  useEffect(() => {
    if (
      !externalNavigationRequest ||
      handledExternalNavigationRef.current === externalNavigationRequest.requestKey
    ) {
      return
    }
    const accepted = requestNavigation(externalNavigationRequest.proceed)
    if (accepted) handledExternalNavigationRef.current = externalNavigationRequest.requestKey
  }, [externalNavigationRequest, requestNavigation])

  useEffect(() => {
    if (drawer.mode !== 'task' || detail.status !== 'ready' || detail.task) return
    const stillListed = automations.tasks.some((task) => task.automationId === drawer.taskId)
    if (!stillListed) {
      setDrawer({ mode: 'closed' })
      setDrawerMaximized(false)
      setDrawerDirty(false)
      showToast(t('automation.updatedElsewhere'))
    }
  }, [automations.tasks, detail.status, detail.task, drawer, showToast, t])

  const openCreate = () => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    requestNavigation(() => {
      drawerOpenerRef.current = opener
      setDrawer({
        mode: 'create',
        draft: createDraft({
          defaultModelId,
          defaultPermissionMode,
          defaultProjectId,
          models
        })
      })
      setDrawerMaximized(false)
      setDrawerDirty(false)
    })
  }

  const openTask = (task: AutomationTask) => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
    requestNavigation(() => {
      drawerOpenerRef.current = opener
      setDrawer({ mode: 'task', taskId: task.automationId, focusRunId: null })
      setDrawerMaximized(false)
      setDrawerDirty(false)
    })
  }

  const closeDrawer = () =>
    requestNavigation(() => {
      setDrawer({ mode: 'closed' })
      setDrawerMaximized(false)
      setDrawerDirty(false)
    })

  const toggleDrawerMaximized = () => {
    setDrawerMaximized((maximized) => (layout.compact ? false : !maximized))
  }

  const openConversation = (conversationId: string, messageId?: string | null) =>
    requestNavigation(() => onOpenConversation(conversationId, messageId))

  const runNow = async (task: AutomationTask) => {
    try {
      await automations.runNow(task)
      showToast(t('automation.runQueued'))
      if (drawer.mode === 'task' && drawer.taskId === task.automationId) void history.refresh()
    } catch (error) {
      showToast(mutationErrorMessage(error, t('automation.runNowFailed'), t), { durationMs: 5000 })
    }
  }

  const setEnabled = async (task: AutomationTask, enabled: boolean) => {
    try {
      await automations.setEnabled(task, enabled)
    } catch (error) {
      showToast(
        mutationErrorMessage(
          error,
          enabled ? t('automation.resumeFailed') : t('automation.pauseFailed'),
          t
        ),
        { durationMs: 5000 }
      )
    }
  }

  const confirmDelete = async () => {
    const target = deleteTarget
    if (!target) return
    try {
      await automations.remove(target)
      if (drawer.mode === 'task' && drawer.taskId === target.automationId) {
        setDrawer({ mode: 'closed' })
        setDrawerMaximized(false)
        setDrawerDirty(false)
      }
      setDeleteTarget(null)
    } catch (error) {
      showToast(mutationErrorMessage(error, t('automation.deleteFailed'), t), { durationMs: 5000 })
      throw error
    }
  }

  const submitDrawer = async (draft: AutomationDraft) => {
    if (drawer.mode === 'create') {
      const created = await automations.create({
        ...draft,
        schedule: withSystemTimeZone(draft.schedule)
      })
      setDrawer({ mode: 'task', taskId: created.automationId, focusRunId: null })
      setDrawerDirty(false)
      return
    }
    if (!selectedTask) throw new Error(t('automation.detailLoadFailed'))
    try {
      const updated = await automations.update(selectedTask, draftToMutation(draft))
      setDrawer({ mode: 'task', taskId: updated.automationId, focusRunId: null })
      setDrawerDirty(false)
    } catch (error) {
      const details = error && typeof error === 'object' ? (error as { code?: unknown }) : null
      if (details?.code === 'revision_conflict') {
        await Promise.all([automations.refresh(), detail.refresh()])
        showToast(t('automation.revisionConflict'), { durationMs: 5000 })
        throw Object.assign(new Error(t('automation.revisionConflict')), {
          code: 'revision_conflict'
        })
      }
      throw error
    }
  }

  const acknowledge = async (attentionId: string) => {
    try {
      await attention.acknowledge(attentionId)
      void automations.refresh()
      void history.refresh()
    } catch (error) {
      showToast(mutationErrorMessage(error, t('automation.rpcUnavailable'), t), {
        durationMs: 5000
      })
    }
  }

  const drawerDraft = useMemo(() => {
    if (drawer.mode === 'create') return drawer.draft
    if (drawer.mode === 'task' && selectedTask) return taskToDraft(selectedTask)
    return createDraft({
      defaultModelId,
      defaultPermissionMode,
      defaultProjectId,
      models
    })
  }, [defaultModelId, defaultPermissionMode, defaultProjectId, drawer, models, selectedTask])

  return (
    <section
      ref={pageRef}
      className="scheduled-page"
      aria-labelledby="scheduled-page-heading"
      data-drawer-open={drawer.mode !== 'closed' || undefined}
      data-drawer-maximized={drawerCoversList || undefined}
      data-layout={layout.compact ? 'compact' : 'split'}
      style={
        {
          '--automation-drawer-width': `${layout.drawerWidth}px`
        } as CSSProperties
      }
    >
      <div
        className="scheduled-page__list-pane"
        aria-hidden={drawerCoversList || undefined}
        inert={drawerCoversList || undefined}
      >
        <header className="scheduled-page__header" data-drag-region>
          <div>
            <h1 id="scheduled-page-heading">{t('automation.pageTitle')}</h1>
            <p>{t('automation.pageDescription')}</p>
          </div>
          <div className="scheduled-page__header-actions" data-no-drag-region>
            {drawer.mode === 'closed' && (
              <button
                ref={createButtonRef}
                type="button"
                className="automation-button automation-button--primary"
                onClick={openCreate}
              >
                <Plus aria-hidden="true" />
                <span>{t('automation.create')}</span>
              </button>
            )}
          </div>
        </header>

        <div className="scheduled-page__toolbar">
          <div className="automation-filters" role="tablist" aria-label={t('automation.pageTitle')}>
            {(
              [
                ['all', 'automation.filterAll', automations.counts.all],
                ['active', 'automation.filterActive', automations.counts.active],
                ['paused', 'automation.filterPaused', automations.counts.paused]
              ] as const
            ).map(([value, key, count]) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={filter === value}
                data-selected={filter === value || undefined}
                onClick={() => setFilter(value)}
              >
                <span>{t(key)}</span>
                <span aria-hidden="true">{count}</span>
              </button>
            ))}
          </div>

          <label className="automation-search">
            <Search aria-hidden="true" />
            <input
              type="search"
              value={query}
              placeholder={t('automation.searchPlaceholder')}
              aria-label={t('automation.searchPlaceholder')}
              onChange={(event) => setQuery(event.currentTarget.value)}
            />
            {query && (
              <button
                type="button"
                aria-label={t('automation.clearSearch')}
                onClick={() => setQuery('')}
              >
                <X aria-hidden="true" />
              </button>
            )}
          </label>
        </div>

        <div
          className="scheduled-page__list"
          aria-live="polite"
          aria-busy={automations.status === 'loading' || undefined}
        >
          {automations.status === 'loading' && automations.tasks.length === 0 ? (
            <div
              className="automation-skeleton-list"
              role="status"
              aria-label={t('automation.loading')}
            >
              {Array.from({ length: 4 }, (_, index) => (
                <div className="automation-skeleton-row" key={index} aria-hidden="true">
                  <span />
                  <div>
                    <span />
                    <span />
                  </div>
                </div>
              ))}
            </div>
          ) : automations.status === 'error' && automations.tasks.length === 0 ? (
            <AutomationPageState
              action={t('automation.retry')}
              icon={<RefreshCw aria-hidden="true" />}
              onAction={() => void automations.refresh()}
              title={t('automation.loadFailedTitle')}
            />
          ) : automations.tasks.length === 0 && query.trim() ? (
            <AutomationPageState
              action={t('automation.clearSearch')}
              icon={<Search aria-hidden="true" />}
              onAction={() => setQuery('')}
              title={t('automation.noResultsTitle')}
              description={t('automation.noResultsDescription')}
            />
          ) : automations.tasks.length === 0 && filter !== 'all' && automations.counts.all > 0 ? (
            <AutomationPageState
              action={t('automation.filterAll')}
              icon={<Clock3 aria-hidden="true" />}
              onAction={() => setFilter('all')}
              title={t('automation.noFilteredTasksTitle')}
              description={t('automation.noFilteredTasksDescription')}
            />
          ) : automations.tasks.length === 0 ? (
            <AutomationPageState
              action={drawer.mode === 'closed' ? t('automation.createTask') : undefined}
              icon={<Clock3 aria-hidden="true" />}
              onAction={drawer.mode === 'closed' ? openCreate : undefined}
              title={t('automation.emptyTitle')}
              description={t('automation.emptyDescription')}
            />
          ) : (
            <>
              <div className="automation-task-list" role="list">
                {automations.tasks.map((task) => (
                  <div role="listitem" key={task.automationId}>
                    <AutomationTaskRow
                      onDelete={setDeleteTarget}
                      onOpen={openTask}
                      onRunNow={(candidate) => void runNow(candidate)}
                      onSetEnabled={(candidate, enabled) => void setEnabled(candidate, enabled)}
                      pending={Boolean(automations.pendingById[task.automationId])}
                      selected={selectedTaskId === task.automationId}
                      task={task}
                    />
                  </div>
                ))}
              </div>
              {automations.nextCursor && (
                <button
                  type="button"
                  className="scheduled-page__load-more"
                  disabled={automations.isLoadingMore}
                  onClick={() => void automations.loadMore()}
                >
                  {automations.isLoadingMore ? t('automation.loading') : t('automation.loadMore')}
                </button>
              )}
            </>
          )}
        </div>
      </div>

      {drawer.mode !== 'closed' && (
        <>
          {!drawerEffectivelyMaximized && layout.drawerResizeMetrics && (
            <AutomationDrawerResizeHandle
              ariaLabel={t('automation.resizeDrawer')}
              metrics={layout.drawerResizeMetrics}
              onResizeCommit={layout.commitDrawerWidth}
              resizeTargetRef={pageRef}
            />
          )}
          <AutomationDrawer
            conversations={conversations}
            detailError={detail.error ? new Error(detail.error.message) : null}
            detailLoading={detail.status === 'loading'}
            draft={drawerDraft}
            focusRunId={drawer.mode === 'task' ? drawer.focusRunId : null}
            historyError={history.error ? new Error(history.error.message) : null}
            historyHasMore={Boolean(history.nextCursor)}
            historyLoading={history.status === 'loading'}
            historyLoadingMore={history.isLoadingMore}
            maximizeDisabled={layout.compact}
            maximized={drawerEffectivelyMaximized}
            mode={drawer.mode}
            models={models}
            mutationPending={navigationBusy}
            onAcknowledgeAttention={(attentionId) => void acknowledge(attentionId)}
            onClose={closeDrawer}
            onDirtyChange={setDrawerDirty}
            onHistoryLoadMore={() => void history.loadMore()}
            onHistoryRetry={() => void history.refresh()}
            onOpenConversation={openConversation}
            onOpenPermissionSettings={onOpenPermissionSettings}
            onRetryDetail={() => void detail.refresh()}
            onSubmittingChange={setFormSubmitting}
            onSubmit={submitDrawer}
            onToggleMaximized={toggleDrawerMaximized}
            permissionModeAvailability={permissionModeAvailability}
            projects={projects}
            runs={history.runs}
            task={selectedTask}
          />
        </>
      )}

      {deleteTarget && (
        <ConfirmationDialog
          cancelLabel={t('automation.cancel')}
          confirmLabel={t('automation.delete')}
          description={t('automation.deleteDescription')}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={confirmDelete}
          title={formatTranslation(t, 'automation.deleteTitle', { title: deleteTarget.title })}
        />
      )}

      {pendingNavigation && (
        <ConfirmationDialog
          cancelLabel={t('automation.cancel')}
          confirmLabel={t('automation.discard')}
          description={t('automation.unsavedDescription')}
          onCancel={() => setPendingNavigation(null)}
          onConfirm={() => {
            const action = pendingNavigation.action
            setPendingNavigation(null)
            setDrawerDirty(false)
            action()
          }}
          title={t('automation.unsavedTitle')}
        />
      )}
    </section>
  )
}

function AutomationPageState({
  action,
  description,
  icon,
  onAction,
  title
}: {
  action?: string
  description?: string
  icon: React.ReactNode
  onAction?: () => void
  title: string
}) {
  return (
    <div className="automation-page-state">
      <span className="automation-page-state__icon">{icon}</span>
      <h2>{title}</h2>
      {description && <p>{description}</p>}
      {action && onAction && (
        <button
          type="button"
          className="automation-button automation-button--secondary"
          onClick={onAction}
        >
          {action}
        </button>
      )}
    </div>
  )
}
