import type { AgentTemplate } from '@mycopilot/protocol'
import { workflowTemplateModelLabel, type WorkflowModelDisplay } from './workflowModelPresentation'
import type { WorkflowText } from './workflowText'
import { WorkflowOptionPicker } from './WorkflowOptionPicker'

/** Template identities stay separate from model selection and retain missing references. */
export function WorkflowTemplatePicker({
  value,
  showSelectedDetail = true,
  templates,
  models,
  text,
  onChange
}: {
  value: string | null
  showSelectedDetail?: boolean
  templates: readonly AgentTemplate[]
  models: readonly WorkflowModelDisplay[]
  text: WorkflowText
  onChange: (templateId: string | null) => void
}) {
  return (
    <WorkflowOptionPicker
      showSelectedDetail={showSelectedDetail}
      value={value}
      ariaLabel={text('template')}
      onChange={onChange}
      options={[
        { id: null, name: text('noTemplate') },
        ...(value && !templates.some((template) => template.templateId === value)
          ? [{ id: value, name: text('missingTemplate'), disabled: true }]
          : []),
        ...templates.map((template) => ({
          id: template.templateId,
          name: template.name,
          detail: workflowTemplateModelLabel(template, models, text)
        }))
      ]}
    />
  )
}
