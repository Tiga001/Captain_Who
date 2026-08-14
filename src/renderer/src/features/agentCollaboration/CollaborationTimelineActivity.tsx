import { Bot } from 'lucide-react'
import { useMemo } from 'react'
import type { ClipboardEvent as ReactClipboardEvent } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation, type Translate } from '../../config/translationFormat'
import {
  groupCollaborationTimelineActivities,
  type CollaborationActivitySemantic,
  type CollaborationTimelineActivity
} from './collaborationTimelineModel'
import './CollaborationTimelineActivity.css'

export {
  groupCollaborationTimelineActivities,
  normalizeCollaborationTimelineActivities
} from './collaborationTimelineModel'
export type {
  CollaborationActivitySemantic,
  CollaborationTimelineActivity,
  CollaborationTimelineActivityGroup
} from './collaborationTimelineModel'

const MAX_VISIBLE_AGENT_CHIPS = 3
const COLLABORATION_COPY_ATTRIBUTE = 'data-collaboration-copy-text'

/**
 * Preserves surrounding selected chat text while replacing semantic rows with an explicit
 * per-Agent status projection. A row-local copy is intercepted only when both selection endpoints
 * are inside that row; broader selections are reconstructed by the Conversation container.
 */
export function copyCollaborationTimelineSelection(event: ReactClipboardEvent<HTMLElement>): void {
  if (event.defaultPrevented) return
  const selection = window.getSelection()
  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return
  const range = selection.getRangeAt(0)
  const root = event.currentTarget
  const copyText = root.getAttribute(COLLABORATION_COPY_ATTRIBUTE)
  if (copyText && root.contains(range.startContainer) && root.contains(range.endContainer)) {
    event.clipboardData.setData('text/plain', copyText)
    event.preventDefault()
    return
  }

  if (!range.intersectsNode(root)) return
  const fragment = range.cloneContents()
  const activityRows = fragment.querySelectorAll<HTMLElement>(`[${COLLABORATION_COPY_ATTRIBUTE}]`)
  if (activityRows.length === 0) return
  for (const row of activityRows) {
    row.replaceWith(document.createTextNode(row.getAttribute(COLLABORATION_COPY_ATTRIBUTE) ?? ''))
  }

  const copySurface = document.createElement('div')
  copySurface.style.cssText =
    'position:fixed;inset:auto auto auto -10000px;white-space:pre-wrap;width:800px'
  copySurface.append(fragment)
  document.body.append(copySurface)
  const selectedText = copySurface.innerText
  copySurface.remove()
  if (!selectedText) return
  event.clipboardData.setData('text/plain', selectedText)
  event.preventDefault()
}

function semanticLabel(semantic: CollaborationActivitySemantic, t: Translate): string {
  switch (semantic) {
    case 'started':
      return t('collaboration.activity.status.started')
    case 'updated':
      return t('collaboration.activity.status.updated')
    case 'waiting_approval':
      return t('collaboration.activity.status.waitingApproval')
    case 'completed':
      return t('collaboration.activity.status.completed')
    case 'failed':
      return t('collaboration.activity.status.failed')
    case 'interrupted':
      return t('collaboration.activity.status.interrupted')
  }
}

export function CollaborationTimelineActivityList({
  activities,
  onOpenAgent
}: {
  activities: readonly CollaborationTimelineActivity[]
  onOpenAgent: (agentId: string) => void
}) {
  const { t } = useFrontendConfig()
  const groups = useMemo(() => groupCollaborationTimelineActivities(activities), [activities])
  if (groups.length === 0) return null

  return (
    <div className="collaboration-timeline" data-testid="collaboration-timeline" role="list">
      {groups.map((group) => {
        const statusLabel = semanticLabel(group.semantic, t)
        const visibleActivities = group.activities.slice(0, MAX_VISIBLE_AGENT_CHIPS)
        const hiddenActivities = group.activities.slice(MAX_VISIBLE_AGENT_CHIPS)
        const copyText = group.activities
          .map((activity) =>
            formatTranslation(t, 'collaboration.activity.copyAgentStatus', {
              name: activity.taskNameSnapshot,
              status: statusLabel
            })
          )
          .join(t('collaboration.activity.statusListSeparator'))
        return (
          <div
            className="collaboration-timeline__activity"
            data-collaboration-copy-text={copyText}
            data-semantic={group.semantic}
            key={group.activities.map((activity) => activity.activityId).join(':')}
            onCopy={copyCollaborationTimelineSelection}
            role="listitem"
          >
            <span className="collaboration-timeline__chips">
              {visibleActivities.map((activity) => (
                <button
                  aria-label={formatTranslation(t, 'collaboration.activity.openAgentActivity', {
                    name: activity.taskNameSnapshot,
                    status: statusLabel
                  })}
                  className="collaboration-timeline__chip"
                  data-agent-id={activity.agentId}
                  key={activity.activityId}
                  onClick={() => onOpenAgent(activity.agentId)}
                  type="button"
                >
                  <Bot aria-hidden="true" />
                  <span>{activity.taskNameSnapshot}</span>
                </button>
              ))}
            </span>
            {hiddenActivities.length > 0 && (
              <span
                aria-label={formatTranslation(t, 'collaboration.activity.moreAgentsLabel', {
                  agents: hiddenActivities
                    .map((activity) => activity.taskNameSnapshot)
                    .join(t('collaboration.activity.agentNameSeparator')),
                  count: String(hiddenActivities.length),
                  status: statusLabel
                })}
                className="collaboration-timeline__overflow"
                role="note"
              >
                {formatTranslation(t, 'collaboration.activity.moreAgents', {
                  count: String(hiddenActivities.length)
                })}
              </span>
            )}
            <span
              aria-atomic="true"
              aria-live="polite"
              className="collaboration-timeline__status"
              data-tone={group.semantic}
            >
              {statusLabel}
            </span>
          </div>
        )
      })}
    </div>
  )
}
