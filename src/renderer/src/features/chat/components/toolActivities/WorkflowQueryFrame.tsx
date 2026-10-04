import type { LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import { queryStatus, type WorkflowQueryProps } from './workflowQueryPresentation'
import './WorkflowQueryToolActivity.css'

export function WorkflowQueryFrame({
  query,
  label,
  icon,
  children,
  hasMessages
}: {
  query: WorkflowQueryProps
  label: string
  icon: LucideIcon
  children: ReactNode
  hasMessages: boolean
}) {
  const status = queryStatus(query)
  const hasDetails = status === 'completed' && hasMessages
  return (
    <AgentActivityDisclosure
      icon={icon}
      label={label}
      hasDetails={hasDetails}
      isPending={status === 'running'}
      className="agent-activity--workflow-query"
    >
      {hasDetails && <div className="workflow-query">{children}</div>}
    </AgentActivityDisclosure>
  )
}
