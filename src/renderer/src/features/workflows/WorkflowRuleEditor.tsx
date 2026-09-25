import { Plus, Trash2 } from 'lucide-react'
import { useId } from 'react'
import type { WorkflowFlow, WorkflowRule } from '@mycopilot/protocol'
import type { WorkflowText } from './workflowText'

interface Props {
  direction: 'input' | 'output'
  flows: WorkflowFlow[]
  rule: WorkflowRule
  flowLabel: (flow: WorkflowFlow) => string
  onChange: (rule: WorkflowRule) => void
  text: WorkflowText
}

export function WorkflowRuleEditor({ direction, flows, rule, flowLabel, onChange, text }: Props) {
  const id = useId()
  const title = text(direction === 'input' ? 'inputRule' : 'outputRule')
  const grouped = new Set(rule.groups.flatMap((group) => group.flowIds))
  return (
    <fieldset className="workflow-rule">
      <legend>{title}</legend>
      <p>{text(direction === 'input' ? 'inputHint' : 'outputHint')}</p>
      <select
        aria-label={title}
        value={rule.mode}
        onChange={(event) =>
          onChange({
            mode: event.currentTarget.value as WorkflowRule['mode'],
            min: Math.min(1, flows.length),
            max: flows.length,
            required: [],
            groups: []
          })
        }
      >
        {(['all', 'one', 'any', 'range', 'custom'] as const).map((mode) => (
          <option key={mode} value={mode}>
            {text(mode)}
          </option>
        ))}
      </select>
      {flows.length === 0 ? <p>{text('noFlows')}</p> : null}
      {rule.mode === 'range' ? (
        <QuantityFields
          min={rule.min}
          max={rule.max}
          limit={flows.length}
          text={text}
          label={title}
          onChange={(values) => onChange({ ...rule, ...values })}
        />
      ) : null}
      {rule.mode === 'custom' ? (
        <>
          <p>{text('groupHint')}</p>
          <div className="workflow-rule__members">
            {flows.map((flow) => (
              <label key={flow.id}>
                <input
                  type="checkbox"
                  checked={rule.required.includes(flow.id)}
                  disabled={grouped.has(flow.id)}
                  onChange={(event) =>
                    onChange({
                      ...rule,
                      required: event.currentTarget.checked
                        ? [...rule.required, flow.id]
                        : rule.required.filter((member) => member !== flow.id)
                    })
                  }
                />
                <span>
                  {flowLabel(flow)} · {text('required')}
                </span>
              </label>
            ))}
          </div>
          {rule.groups.map((group, index) => {
            const unavailable = new Set([
              ...rule.required,
              ...rule.groups
                .filter((other) => other.id !== group.id)
                .flatMap((other) => other.flowIds)
            ])
            const update = (values: Partial<typeof group>) =>
              onChange({
                ...rule,
                groups: rule.groups.map((existing) =>
                  existing.id === group.id ? { ...existing, ...values } : existing
                )
              })
            return (
              <fieldset className="workflow-rule__group" key={group.id}>
                <legend>
                  {text('groups')} {index + 1}
                </legend>
                <div className="workflow-rule__members">
                  {flows.map((flow) => (
                    <label key={flow.id}>
                      <input
                        type="checkbox"
                        checked={group.flowIds.includes(flow.id)}
                        disabled={unavailable.has(flow.id)}
                        onChange={(event) =>
                          update({
                            flowIds: event.currentTarget.checked
                              ? [...group.flowIds, flow.id]
                              : group.flowIds.filter((member) => member !== flow.id)
                          })
                        }
                      />
                      <span>{flowLabel(flow)}</span>
                    </label>
                  ))}
                </div>
                <QuantityFields
                  min={group.min}
                  max={group.max}
                  limit={group.flowIds.length}
                  label={`${title} ${text('groups')} ${index + 1}`}
                  text={text}
                  onChange={update}
                />
                <button
                  type="button"
                  className="workflow-button workflow-button--quiet"
                  onClick={() =>
                    onChange({
                      ...rule,
                      groups: rule.groups.filter((existing) => existing.id !== group.id)
                    })
                  }
                >
                  <Trash2 aria-hidden="true" />
                  {text('removeGroup')}
                </button>
              </fieldset>
            )
          })}
          <button
            type="button"
            className="workflow-button"
            aria-describedby={`${id}-hint`}
            disabled={flows.length === 0}
            onClick={() =>
              onChange({
                ...rule,
                groups: [...rule.groups, { id: crypto.randomUUID(), flowIds: [], min: 1, max: 1 }]
              })
            }
          >
            <Plus aria-hidden="true" />
            {text('addGroup')}
          </button>
          <span className="workflow-sr-only" id={`${id}-hint`}>
            {text('groupHint')}
          </span>
        </>
      ) : null}
    </fieldset>
  )
}

function QuantityFields({
  min,
  max,
  limit,
  label,
  text,
  onChange
}: {
  min: number
  max: number
  limit: number
  label: string
  text: WorkflowText
  onChange: (values: { min: number } | { max: number }) => void
}) {
  return (
    <div className="workflow-quantities">
      <label>
        <span>{text('min')}</span>
        <input
          aria-label={`${label} ${text('min')}`}
          type="number"
          min={0}
          max={limit}
          step={1}
          value={min}
          onChange={(event) =>
            onChange({ min: Math.max(0, Math.floor(Number(event.currentTarget.value) || 0)) })
          }
        />
      </label>
      <label>
        <span>{text('max')}</span>
        <input
          aria-label={`${label} ${text('max')}`}
          type="number"
          min={0}
          max={limit}
          step={1}
          value={max}
          onChange={(event) =>
            onChange({ max: Math.max(0, Math.floor(Number(event.currentTarget.value) || 0)) })
          }
        />
      </label>
    </div>
  )
}
