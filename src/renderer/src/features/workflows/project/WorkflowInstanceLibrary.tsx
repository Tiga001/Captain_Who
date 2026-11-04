import type { WorkflowActivity, WorkflowInstance } from '@mycopilot/protocol'
import { Network, Settings2, Trash2 } from 'lucide-react'
import type { CSSProperties } from 'react'
import { Tooltip } from '../../../components/overlay/Tooltip'
import type { ChatConversation } from '../../chat/chatTypes'
import { WorkflowActivityElapsed } from './WorkflowActivityElapsed'
import { projectWorkflowText } from './projectWorkflowText'

interface WorkflowInstanceLibraryProps {
  activeConversations: readonly ChatConversation[]
  activityByInstanceId: ReadonlyMap<string, WorkflowActivity | null>
  busy: boolean
  colorOccupiedMessage: string
  instances: readonly WorkflowInstance[]
  language: string
  onConfigure: (instance: WorkflowInstance) => void
  onCreate: () => void
  onOpenMonitor: (instanceId: string) => void
  onRequestRemove: (instance: WorkflowInstance) => void
  onToggleEnabled: (instance: WorkflowInstance) => Promise<void>
  pendingSync: boolean
  runningInstanceIds: ReadonlySet<string>
  togglingId: string | null
}

/** Cards derive display state only; the page owns all mutation guards and live activity. */
export function WorkflowInstanceLibrary({
  activeConversations,
  activityByInstanceId,
  busy,
  colorOccupiedMessage,
  instances,
  language,
  onConfigure,
  onCreate,
  onOpenMonitor,
  onRequestRemove,
  onToggleEnabled,
  pendingSync,
  runningInstanceIds,
  togglingId
}: WorkflowInstanceLibraryProps) {
  const t = projectWorkflowText(language)

  return (
    <div className="project-workflows__library">
      {instances.map((instance) => {
        const complete = instance.definition.nodes
          .filter((node) => node.kind === 'agent')
          .every((node) =>
            instance.bindings.some(
              (binding) =>
                binding.nodeId === node.id &&
                activeConversations.some(
                  (conversation) => conversation.id === binding.conversationId
                )
            )
          )
        const conflicts = instances.some(
          (other) =>
            other.id !== instance.id &&
            other.enabled &&
            other.color.toLowerCase() === instance.color.toLowerCase()
        )
        const enableBlocked = !instance.enabled && (!complete || conflicts)
        const switchTitle = instance.enabled
          ? t('停用组织', 'Disable organization')
          : conflicts
            ? colorOccupiedMessage
            : enableBlocked
              ? t('请先补全成员的对话分配', 'Complete member conversation assignments first')
              : t('启用组织', 'Enable organization')
        return (
          <article
            className={`project-workflows__instance${instance.enabled && runningInstanceIds.has(instance.id) ? ' is-running' : ''}`}
            key={instance.id}
            style={{ '--workflow-color': instance.color } as CSSProperties}
          >
            <svg className="project-workflows__activity" aria-hidden="true" focusable="false">
              <rect className="project-workflows__activity-track" pathLength="100" />
              <rect className="project-workflows__activity-trail" pathLength="100" />
              <rect className="project-workflows__activity-head" pathLength="100" />
            </svg>
            <span className="project-workflows__color-dot" style={{ background: instance.color }} />
            <button
              type="button"
              className="project-workflows__instance-main"
              aria-disabled={togglingId === instance.id || undefined}
              disabled={busy || !!pendingSync}
              onClick={() => onConfigure(instance)}
            >
              <strong>{instance.name}</strong>
              <small className="project-workflows__instance-meta">
                <span>
                  {t(
                    `${instance.bindings.length} 个对话`,
                    `${instance.bindings.length} conversations`
                  )}
                </span>
                <WorkflowActivityElapsed
                  activity={activityByInstanceId.get(instance.id)}
                  language={language}
                />
              </small>
            </button>
            <Tooltip content={switchTitle}>
              <button
                type="button"
                role="switch"
                className="project-workflows__switch"
                aria-label={`${instance.enabled ? t('停用组织', 'Disable organization') : t('启用组织', 'Enable organization')} ${instance.name}`}
                aria-checked={instance.enabled}
                aria-busy={togglingId === instance.id || undefined}
                aria-disabled={!!togglingId || undefined}
                disabled={busy || !!pendingSync || enableBlocked}
                onClick={() => void onToggleEnabled(instance)}
              >
                <span aria-hidden="true" />
              </button>
            </Tooltip>
            <Tooltip content={t('组织看板', 'Organization board')}>
              <button
                type="button"
                className="workflow-icon-button project-workflows__diagram"
                aria-label={`${t('组织看板', 'Organization board')} ${instance.name}`}
                onClick={() => onOpenMonitor(instance.id)}
              >
                <Network aria-hidden="true" />
              </button>
            </Tooltip>
            <Tooltip content={t('配置', 'Configure')}>
              <button
                type="button"
                className="workflow-icon-button project-workflows__configure"
                aria-label={`${t('配置', 'Configure')} ${instance.name}`}
                aria-disabled={togglingId === instance.id || undefined}
                disabled={busy || !!pendingSync}
                onClick={() => onConfigure(instance)}
              >
                <Settings2 aria-hidden="true" />
              </button>
            </Tooltip>
            <Tooltip content={t('移除组织', 'Remove organization')}>
              <button
                type="button"
                className="workflow-icon-button"
                aria-label={`${t('移除组织', 'Remove organization')} ${instance.name}`}
                aria-disabled={!!togglingId || undefined}
                disabled={busy || instance.running || !!pendingSync}
                onClick={() => {
                  onRequestRemove(instance)
                }}
              >
                <Trash2 />
              </button>
            </Tooltip>
          </article>
        )
      })}
      {!instances.length && (
        <div className="project-workflows__empty">
          <Network />
          <h2>{t('让对话一起协作', 'Bring conversations together')}</h2>
          <p>
            {t(
              '从模板创建组织，再为智能体绑定对话。',
              'Create an organization from a template, then bind conversations to its agents.'
            )}
          </p>
          <button
            type="button"
            className="workflow-button workflow-button--primary"
            disabled={busy || !!pendingSync}
            onClick={() => onCreate()}
          >
            <Settings2 aria-hidden="true" />
            {t('激活新组织', 'Activate new organization')}
          </button>
        </div>
      )}
    </div>
  )
}
