import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import { AgentActivityDisclosure } from './AgentActivityDisclosure'
import {
  WorkflowAcceptIcon,
  WorkflowCompleteIcon,
  WorkflowRecallIcon
} from './WorkflowMailboxIcons'
import { WorkflowMessageBox } from './WorkflowSendToolActivity'
import { record, records, text, type WorkflowQueryProps } from './workflowQueryPresentation'

export function WorkflowMailActionToolActivity({
  call,
  result,
  cancelled,
  settledStatus
}: WorkflowQueryProps) {
  const { language } = useFrontendConfig(),
    chinese = language.startsWith('zh')
  const kind =
    call.tool === 'workflow_accept'
      ? 'accept'
      : call.tool === 'workflow_complete'
        ? 'complete'
        : 'recall'
  const data = record(result?.result),
    messages = records(data.messages)
  const action = {
    accept: { zh: '接手', en: 'Accept', done: 'Accepted', icon: WorkflowAcceptIcon },
    complete: { zh: '标记已处理', en: 'Complete', done: 'Completed', icon: WorkflowCompleteIcon },
    recall: { zh: '撤回', en: 'Recall', done: 'Recalled', icon: WorkflowRecallIcon }
  }[kind]
  const requested = Array.isArray(record(call.args).messageIds)
    ? (record(call.args).messageIds as unknown[]).length
    : 0
  const count = messages.length || requested
  const successful = messages.filter((message) => message.success === true).length
  const unsuccessful = messages.filter((message) => message.success === false).length
  const pending = !result && !cancelled && !settledStatus
  const interrupted = !result && (cancelled || settledStatus === 'cancelled')
  const failed = result
    ? result.ok === false || (messages.length > 0 && unsuccessful === messages.length)
    : settledStatus === 'failed'
  const success = Boolean(result?.ok && successful > 0)
  const number = success ? successful : count
  const subject = chinese ? `工作流中的 ${number} 条消息` : `${number} workflow messages`
  const done = kind === 'complete' ? `已将${subject}标记为已处理` : `已${action.zh}${subject}`
  const progress = { accept: 'Accepting', complete: 'Completing', recall: 'Recalling' }[kind]
  let label = chinese
    ? pending
      ? `正在${action.zh}${subject}`
      : failed
        ? `${action.zh}${subject}失败`
        : interrupted
          ? `已取消${action.zh}${subject}`
          : success
            ? done
            : `${action.zh}${subject}的结果待确认`
    : pending
      ? `${progress} ${subject}`
      : failed
        ? `Could not ${action.en.toLowerCase()} ${subject}`
        : interrupted
          ? `Cancelled ${action.en.toLowerCase()}`
          : success
            ? `${action.done} ${subject}`
            : `${action.en} result is unconfirmed`
  if (success && unsuccessful)
    label += chinese ? ` · ${unsuccessful} 条未完成` : ` · ${unsuccessful} unsuccessful`
  return (
    <AgentActivityDisclosure
      icon={action.icon}
      label={label}
      isPending={pending}
      hasDetails={messages.length > 0 || Boolean(result?.error)}
      className="agent-activity--workflow-mail-action"
    >
      <div className="workflow-send-messages">
        {messages.map((row, index) => {
          const message = { ...record(row.message), ...row }
          const name =
            text(kind === 'recall' ? message.targetNodeName : message.sourceNodeName) ||
            (chinese ? '工作流消息' : 'Workflow message')
          const conversationId = text(
            kind === 'recall' ? message.targetConversationId : message.sourceConversationId
          )
          return (
            <div key={text(message.messageId) || text(message.id) || index}>
              <WorkflowMessageBox
                name={name}
                message={text(message.content)}
                conversationId={conversationId}
                chinese={chinese}
              />
              {text(message.error) && (
                <p className="workflow-send-messages__error">{text(message.error)}</p>
              )}
            </div>
          )
        })}
        {result?.error && <p className="workflow-send-messages__error">{result.error}</p>}
      </div>
    </AgentActivityDisclosure>
  )
}
