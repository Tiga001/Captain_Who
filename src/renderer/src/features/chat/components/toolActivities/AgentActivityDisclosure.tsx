import { ChevronDown, type LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'

interface AgentActivityDisclosureProps {
  children?: ReactNode
  className?: string
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
  hasDetails,
  icon: Icon,
  iconBadge,
  iconBadgeTone,
  isPending = false,
  label
}: AgentActivityDisclosureProps) {
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
    <details className={activityClassName}>
      <summary>
        {iconNode}
        {labelNode}
        <ChevronDown className="agent-activity__chevron" aria-hidden="true" />
      </summary>
      {children}
    </details>
  )
}
