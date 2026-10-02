import { Inbox } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
export function WorkflowNodeMailbox({ count }: { count: number }) {
  const { language } = useFrontendConfig()
  return (
    <span
      className="workflow-node__mailbox"
      role="img"
      aria-label={language.startsWith('zh') ? `${count} 封未处理邮件` : `${count} pending messages`}
    >
      <Inbox size={19} />
      {count > 0 && <span className="workflow-node__mailbox-count">{count}</span>}
    </span>
  )
}
