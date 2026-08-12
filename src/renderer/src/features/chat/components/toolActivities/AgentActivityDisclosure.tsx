import { ChevronDown, type LucideIcon } from 'lucide-react'
import { useState, type ReactNode } from 'react'

interface AgentActivityDisclosureProps {
  children?: ReactNode
  className?: string
  defaultOpen?: boolean
  hasDetails: boolean
  icon: LucideIcon
  iconBadge?: ReactNode
  iconBadgeTone?: 'danger' | 'blocked'
  isPending?: boolean
  label: string
  revealDetailsOnOpen?: boolean
}

function revealExpandedDetails(activity: HTMLDetailsElement) {
  const scrollContainer = activity.closest<HTMLElement>('.chat-conversation-page__messages')
  if (!scrollContainer) return

  window.requestAnimationFrame(() => {
    const containerRect = scrollContainer.getBoundingClientRect()
    const activityRect = activity.getBoundingClientRect()
    const viewportInset = 16
    const visibleBottom = containerRect.bottom - viewportInset
    const bottomOverflow = activityRect.bottom - visibleBottom
    if (bottomOverflow <= 0) return

    const availableHeight = Math.max(0, containerRect.height - viewportInset * 2)
    const scrollDelta =
      activityRect.height <= availableHeight
        ? bottomOverflow
        : Math.max(0, activityRect.top - containerRect.top - viewportInset)
    const maximumScrollTop = Math.max(
      0,
      scrollContainer.scrollHeight - scrollContainer.clientHeight
    )

    // Expanded command details can grow below the message viewport; keep them above the composer.
    scrollContainer.scrollTo({
      behavior: window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
      top: Math.min(maximumScrollTop, scrollContainer.scrollTop + scrollDelta)
    })
  })
}

export function AgentActivityDisclosure({
  children,
  className,
  defaultOpen = false,
  hasDetails,
  icon: Icon,
  iconBadge,
  iconBadgeTone,
  isPending = false,
  label,
  revealDetailsOnOpen = false
}: AgentActivityDisclosureProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen)
  const activityClassName = ['agent-activity', className].filter(Boolean).join(' ')
  const labelClassName = ['agent-activity__label', isPending ? 'agent-running-text' : '']
    .filter(Boolean)
    .join(' ')
  const labelNode = <span className={labelClassName}>{label}</span>
  const iconNode = (
    <span className="agent-activity__icon">
      <Icon aria-hidden="true" />
      {iconBadge ? (
        <span className="agent-activity__icon-badge" data-tone={iconBadgeTone}>
          {iconBadge}
        </span>
      ) : null}
    </span>
  )

  if (!hasDetails) {
    return (
      <div className={activityClassName}>
        <div className="agent-activity__static-summary">
          {iconNode}
          {labelNode}
        </div>
      </div>
    )
  }

  return (
    <details
      className={activityClassName}
      onToggle={(event) => {
        const details = event.currentTarget
        setIsOpen(details.open)
        if (details.open && revealDetailsOnOpen) revealExpandedDetails(details)
      }}
      open={isOpen}
    >
      <summary>
        {iconNode}
        {labelNode}
        <ChevronDown className="agent-activity__chevron" aria-hidden="true" />
      </summary>
      {children}
    </details>
  )
}
