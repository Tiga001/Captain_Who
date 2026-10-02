import { Network } from 'lucide-react'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import {
  queryLabel,
  queryStatus,
  record,
  text,
  type Localize,
  type WorkflowQueryProps
} from './workflowQueryPresentation'

export function WorkflowStateToolActivity(props: WorkflowQueryProps) {
  const { language } = useFrontendConfig()
  const chinese = language === 'zh-CN' || language === 'zh-TW'
  const l: Localize = (zh, en) => (chinese ? zh : en)
  const reason = (text(record(props.call.args).reason) || text(props.call.reason)).trim()
  const label = queryLabel(props, l('组织', 'organization'), l)
  return (
    <AgentActivityDisclosure
      icon={Network}
      label={reason ? `${label} · ${reason}` : label}
      hasDetails={false}
      isPending={queryStatus(props) === 'running'}
      className="agent-activity--workflow-query"
    />
  )
}
