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
  label
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
      onToggle={(event) => setIsOpen(event.currentTarget.open)}
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
