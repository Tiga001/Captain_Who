import {
  AlertCircle,
  Clock3,
  LoaderCircle,
  MessageCircleMore,
  MoreHorizontal,
  PauseCircle,
  Play,
  Trash2
} from 'lucide-react'
import { useCallback, useRef, useState } from 'react'
import type { AutomationTask } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import {
  formatAbsoluteDateTime,
  isTaskRunActive,
  taskSecondaryText
} from '../automationPresentation'

interface AutomationTaskRowProps {
  onDelete: (task: AutomationTask) => void
  onOpen: (task: AutomationTask) => void
  onRunNow: (task: AutomationTask) => void
  onSetEnabled: (task: AutomationTask, enabled: boolean) => void
  pending?: boolean
  selected?: boolean
  task: AutomationTask
}

export function AutomationTaskRow({
  onDelete,
  onOpen,
  onRunNow,
  onSetEnabled,
  pending = false,
  selected = false,
  task
}: AutomationTaskRowProps) {
  const { language, t } = useFrontendConfig()
  const [menuOpen, setMenuOpen] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const closeMenu = useCallback(() => setMenuOpen(false), [])
  useDismissOnOutsidePointer(menuRef, menuOpen, closeMenu)
  const runActive = isTaskRunActive(task)
  const TaskIcon = task.destination.kind === 'new_chat' ? Clock3 : MessageCircleMore
  const status =
    task.health.state === 'blocked' ? 'blocked' : (task.latestRun?.status ?? task.status)
  const StatusIcon =
    task.health.state === 'blocked' ||
    task.latestRun?.status === 'failed' ||
    task.latestRun?.status === 'waiting_for_approval'
      ? AlertCircle
      : runActive
        ? LoaderCircle
        : task.status === 'paused'
          ? Play
          : TaskIcon

  return (
    <article
      className="automation-task-row"
      data-attention={Boolean(task.attention && task.attention.readAt === null) || undefined}
      data-selected={selected || undefined}
      data-status={status}
    >
      <button
        type="button"
        className="automation-task-row__main"
        aria-current={selected ? 'true' : undefined}
        onClick={() => onOpen(task)}
      >
        <span
          className="automation-task-row__status-icon"
          aria-label={
            task.health.state === 'blocked'
              ? t('automation.statusBlocked')
              : runActive
                ? t('automation.statusRunning')
                : task.status === 'paused'
                  ? t('automation.statusPaused')
                  : t('automation.statusActive')
          }
        >
          <StatusIcon aria-hidden="true" />
        </span>
        <span className="automation-task-row__content">
          <strong>{task.title}</strong>
          <span
            title={
              task.nextRunAt === null
                ? undefined
                : `${formatAbsoluteDateTime(task.nextRunAt, language, task.timezone)} · ${task.timezone}`
            }
          >
            {taskSecondaryText(task, t, language)}
          </span>
        </span>
        {task.attention && task.attention.readAt === null && (
          <span
            className="automation-task-row__attention"
            aria-label={t('automation.requiresAttention')}
          />
        )}
      </button>

      <div className="automation-task-row__menu-root" ref={menuRef}>
        <button
          type="button"
          className="automation-icon-button"
          aria-label={`${t('automation.moreActions')}: ${task.title}`}
          aria-expanded={menuOpen}
          aria-haspopup="menu"
          disabled={pending}
          onClick={(event) => {
            event.stopPropagation()
            setMenuOpen((open) => !open)
          }}
        >
          {pending ? (
            <LoaderCircle className="automation-spinning" aria-hidden="true" />
          ) : (
            <MoreHorizontal aria-hidden="true" />
          )}
        </button>
        {menuOpen && (
          <div
            className="automation-task-menu"
            role="menu"
            aria-label={task.title}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault()
                closeMenu()
              }
            }}
          >
            <button
              type="button"
              role="menuitem"
              disabled={runActive || pending || task.health.state === 'blocked'}
              onClick={() => {
                closeMenu()
                onRunNow(task)
              }}
            >
              <Play aria-hidden="true" />
              <span>{runActive ? t('automation.runningAction') : t('automation.runNow')}</span>
            </button>
            <button
              type="button"
              role="menuitem"
              disabled={pending}
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
              disabled={pending}
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
    </article>
  )
}
