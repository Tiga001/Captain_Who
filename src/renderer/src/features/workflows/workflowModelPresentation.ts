import type { AgentTemplate, WorkflowNode } from '@mycopilot/protocol'
import type { ModelConfig } from '../../config/modelConfig'
import { formatModelConfigLabel } from '../modelSelection/modelConfigPresentation'
import type { WorkflowText } from './workflowText'

export type WorkflowModelDisplay = Pick<ModelConfig, 'id' | 'displayName' | 'execution'>

function modelLabel(
  id: string | null,
  models: readonly WorkflowModelDisplay[],
  text: WorkflowText,
  storedLabel?: string | null
): string {
  if (!id) return text('modelNotSelected')
  const model = models.find((candidate) => candidate.id === id)
  const label = (model ? formatModelConfigLabel(model) : storedLabel?.trim()) || ''
  if (model?.execution.status === 'available') return label || text('modelUnavailable')
  return label ? `${label} · ${text('modelUnavailable')}` : text('modelUnavailable')
}

export function workflowTemplateModelLabel(
  template: AgentTemplate,
  models: readonly WorkflowModelDisplay[],
  text: WorkflowText
): string {
  return modelLabel(template.modelConfigId, models, text, template.modelDisplayName)
}

export function workflowNodeModelLabel(
  node: WorkflowNode,
  templates: readonly AgentTemplate[],
  models: readonly WorkflowModelDisplay[],
  text: WorkflowText
): string {
  if (node.templateId) {
    const template = templates.find((candidate) => candidate.templateId === node.templateId)
    return template ? workflowTemplateModelLabel(template, models, text) : text('missingTemplate')
  }
  return modelLabel(node.modelConfigId ?? null, models, text)
}
