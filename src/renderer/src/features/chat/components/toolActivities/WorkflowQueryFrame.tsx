import { ArrowUpRight, type LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { useWorkflowNavigation } from '../../../workflows/WorkflowNavigationContext'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import {
  queryStatus,
  record,
  text,
  timeLabel,
  type Localize,
  type WorkflowQueryProps
} from './workflowQueryPresentation'
import './WorkflowQueryToolActivity.css'

export function WorkflowQueryFrame({
  query,
  label,
  icon,
  children,
  l,
  hasMessages
}: {
  query: WorkflowQueryProps
  label: string
  icon: LucideIcon
  children: ReactNode
  l: Localize
  hasMessages: boolean
}) {
  const { language } = useFrontendConfig()
  const openWorkflow = useWorkflowNavigation()
  const status = queryStatus(query)
  const data = record(query.result?.result)
  const time = timeLabel(data.observedAt, language)
  const hasDetails = status === 'completed' && hasMessages
  return (
    <AgentActivityDisclosure
      icon={icon}
      label={label}
      hasDetails={hasDetails}
      isPending={status === 'running'}
      className="agent-activity--workflow-query"
    >
      {hasDetails && (
        <div className="workflow-query">
          <div className="workflow-query__header">
            <span>
              {text(data.workflowName) || l('工作流', 'Workflow')}
              <small>
                {l('查询快照', 'Snapshot')}
                {time && ` · ${time}`}
              </small>
            </span>
            {text(data.instanceId) && openWorkflow && (
              <button type="button" onClick={() => openWorkflow(text(data.instanceId))}>
                <ArrowUpRight aria-hidden="true" />
                {l('打开工作流', 'Open workflow')}
              </button>
            )}
          </div>
          {children}
        </div>
      )}
    </AgentActivityDisclosure>
  )
}
