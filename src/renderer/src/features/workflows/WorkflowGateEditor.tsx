import type { WorkflowFlow, WorkflowGateNode } from '@mycopilot/protocol'
import type { WorkflowText } from './workflowText'
import { WorkflowRuleEditor } from './WorkflowRuleEditor'
import { WorkflowOptionPicker } from './WorkflowOptionPicker'

export function WorkflowGateEditor({
  node,
  flows,
  text,
  flowLabel,
  recipient = 'agent',
  onChange
}: {
  node: WorkflowGateNode
  flows: WorkflowFlow[]
  text: WorkflowText
  flowLabel: (flow: WorkflowFlow) => string
  recipient?: 'agent' | 'user'
  onChange: (node: WorkflowGateNode) => void
}) {
  if (node.kind === 'outputGate')
    return (
      <WorkflowRuleEditor
        flows={flows}
        rule={node.selection}
        text={text}
        flowLabel={flowLabel}
        onChange={(selection) => onChange({ ...node, selection })}
      />
    )
  return (
    <>
      <div className="workflow-gate-settings__arrival">
        <span>{text('processingMode')}</span>
        <WorkflowOptionPicker
          ariaLabel={text('processingMode')}
          value={node.processingMode}
          showSelectedDetail={false}
          options={[
            { id: 'individual', name: text('individual'), detail: text('individualHint') },
            { id: 'batch', name: text('batch'), detail: text('batchHint') }
          ]}
          onChange={(value) =>
            onChange({ ...node, processingMode: value === 'individual' ? 'individual' : 'batch' })
          }
        />
        <p>{text(node.processingMode === 'individual' ? 'individualHint' : 'batchHint')}</p>
      </div>
      <div className="workflow-gate-settings__arrival">
        <span>{text(recipient === 'user' ? 'userBusyPolicy' : 'busyPolicy')}</span>
        <WorkflowOptionPicker
          ariaLabel={text(recipient === 'user' ? 'userBusyPolicy' : 'busyPolicy')}
          value={node.busyPolicy}
          showSelectedDetail={false}
          options={[
            { id: 'queue', name: text('queue'), detail: text('queueHint') },
            { id: 'inject', name: text('inject'), detail: text('injectHint') }
          ]}
          onChange={(value) =>
            onChange({ ...node, busyPolicy: value === 'inject' ? 'inject' : 'queue' })
          }
        />
        <p>{text(node.busyPolicy === 'queue' ? 'queueHint' : 'injectHint')}</p>
      </div>
    </>
  )
}
