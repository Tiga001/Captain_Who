import { ChevronLeft, ChevronRight } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { AgentAvatar } from '../../agentCollaboration/AgentAvatar'
import './ConversationApprovalQueue.css'

export interface ConversationApprovalQueueItem {
  /** Stable approval identity, rather than the identity of its source Agent. */
  id: string
  sourceAgentId?: string
  label: string
  content: ReactNode
}

interface ConversationApprovalQueueProps {
  items: readonly ConversationApprovalQueueItem[]
  onOpenAgent?: (agentId: string) => void
}

/** One visible approval, with every pending card retained until its decision is settled. */
export function ConversationApprovalQueue({ items, onOpenAgent }: ConversationApprovalQueueProps) {
  const { t } = useFrontendConfig()
  const [selection, setSelection] = useState(() => ({ id: items[0]?.id, index: 0 }))
  const retainedIndex = items.findIndex((item) => item.id === selection.id)
  const currentIndex =
    retainedIndex >= 0 ? retainedIndex : Math.max(0, Math.min(selection.index, items.length - 1))
  const current = items[currentIndex]

  // Reconcile before committing a frame: new arrivals retain the current approval; removing it
  // selects the next item at its position, or the previous item when the removed item was last.
  if (selection.id !== current?.id || selection.index !== currentIndex) {
    setSelection({ id: current?.id, index: currentIndex })
  }

  if (!current) return null

  const showHeader = items.length > 1 || Boolean(current.sourceAgentId)
  const source = (
    <>
      {current.sourceAgentId ? <AgentAvatar agentId={current.sourceAgentId} /> : null}
      <span className="conversation-approval-queue__name">{current.label}</span>
    </>
  )

  return (
    <div className="conversation-approval-queue">
      {showHeader ? (
        <div className="conversation-approval-queue__header">
          {current.sourceAgentId && onOpenAgent ? (
            <button
              aria-label={t('collaboration.approval.openAgent').replace('{task}', current.label)}
              className="conversation-approval-queue__source"
              onClick={() => onOpenAgent(current.sourceAgentId!)}
              title={current.label}
              type="button"
            >
              {source}
            </button>
          ) : (
            <div className="conversation-approval-queue__source" title={current.label}>
              {source}
            </div>
          )}
          <nav
            aria-label={t('agent.approval.navigation.label')}
            className="conversation-approval-queue__navigation"
          >
            <button
              aria-label={t('agent.approval.navigation.previous')}
              disabled={currentIndex === 0}
              onClick={() =>
                setSelection({ id: items[currentIndex - 1].id, index: currentIndex - 1 })
              }
              type="button"
            >
              <ChevronLeft aria-hidden="true" />
            </button>
            <span aria-atomic="true" aria-live="polite">
              {currentIndex + 1} / {items.length}
            </span>
            <button
              aria-label={t('agent.approval.navigation.next')}
              disabled={currentIndex === items.length - 1}
              onClick={() =>
                setSelection({ id: items[currentIndex + 1].id, index: currentIndex + 1 })
              }
              type="button"
            >
              <ChevronRight aria-hidden="true" />
            </button>
          </nav>
        </div>
      ) : null}
      {items.map((item) => (
        <div
          className="conversation-approval-queue__item"
          data-approval-queue-id={item.id}
          hidden={item.id !== current.id}
          inert={item.id !== current.id}
          key={item.id}
        >
          {item.content}
        </div>
      ))}
    </div>
  )
}
