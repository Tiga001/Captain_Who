import type { WorkflowDefinition, WorkflowGateNode } from '@mycopilot/protocol'
import type { WorkflowText } from '../workflowText'

export function WorkflowGateTooltip({
  node,
  graph,
  text
}: {
  node: WorkflowGateNode
  graph: WorkflowDefinition
  text: WorkflowText
}) {
  const flows = graph.flows.filter(
    (flow) => flow.source.kind === 'node' && flow.source.nodeId === node.id
  )
  const flowLabel = (id: string) => {
    const flow = flows.find((item) => item.id === id)
    if (!flow) return id
    const target = flow.target
    const recipient =
      target.kind === 'node' ? graph.nodes.find((item) => item.id === target.nodeId) : undefined
    return [flow.name || id, recipient?.name].filter(Boolean).join(' → ')
  }
  const userRecipient = flows.some((flow) => {
    const target = flow.target
    return (
      target.kind === 'node' &&
      graph.nodes.some((item) => item.id === target.nodeId && item.kind === 'user')
    )
  })
  const rule = node.kind === 'outputGate' ? node.selection : null
  const members = (ids: string[]) => (ids.length ? ids.map(flowLabel).join('、') : '—')
  return (
    <div className="workflow-binding-node-tooltip workflow-monitor__gate-tooltip">
      <strong>{node.name || text(node.kind)}</strong>
      <dl>
        <div>
          <dt>{text('gateType')}</dt>
          <dd>{text(node.kind)}</dd>
        </div>
        {node.kind === 'inputGate' ? (
          <>
            <div>
              <dt>{text('processingMode')}</dt>
              <dd>{text(node.processingMode)}</dd>
            </div>
            <div className="workflow-binding-node-tooltip__task">
              <dd>{text(node.processingMode === 'batch' ? 'batchHint' : 'individualHint')}</dd>
            </div>
            <div>
              <dt>{text(userRecipient ? 'userBusyPolicy' : 'busyPolicy')}</dt>
              <dd>{text(node.busyPolicy)}</dd>
            </div>
            <div className="workflow-binding-node-tooltip__task">
              <dd>{text(node.busyPolicy === 'queue' ? 'queueHint' : 'injectHint')}</dd>
            </div>
          </>
        ) : null}
        {rule ? (
          <>
            <div>
              <dt>{text('outputRule')}</dt>
              <dd>{text(rule.mode)}</dd>
            </div>
            {rule.mode === 'exact' && (
              <div>
                <dt>{text('quantity')}</dt>
                <dd>{rule.min}</dd>
              </div>
            )}
            {rule.mode === 'range' && (
              <>
                <div>
                  <dt>{text('min')}</dt>
                  <dd>{rule.min}</dd>
                </div>
                <div>
                  <dt>{text('max')}</dt>
                  <dd>{rule.max}</dd>
                </div>
              </>
            )}
            {rule.mode === 'custom' ? (
              <>
                <div className="workflow-binding-node-tooltip__task">
                  <dt>{text('required')}</dt>
                  <dd>{members(rule.required)}</dd>
                </div>
                {rule.groups.map((group, index) => (
                  <div key={group.id} className="workflow-binding-node-tooltip__task">
                    <dt>
                      {text('groups')} {index + 1} · {group.min}–{group.max}
                    </dt>
                    <dd>{members(group.flowIds)}</dd>
                  </div>
                ))}
                <div className="workflow-binding-node-tooltip__task">
                  <dt>{text('optional')}</dt>
                  <dd>
                    {members(
                      flows
                        .map((flow) => flow.id)
                        .filter(
                          (id) =>
                            !rule.required.includes(id) &&
                            !rule.groups.some((group) => group.flowIds.includes(id))
                        )
                    )}
                  </dd>
                </div>
              </>
            ) : (
              <div className="workflow-binding-node-tooltip__task">
                <dt>{text('target')}</dt>
                <dd>{members(flows.map((flow) => flow.id))}</dd>
              </div>
            )}
          </>
        ) : null}
      </dl>
    </div>
  )
}
