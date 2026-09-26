import type {
  AgentTemplate,
  AgentTemplateList,
  AgentTemplateListRequest,
  AgentTemplateCreateRequest,
  AgentTemplateUpdateRequest,
  AgentTemplateSetEnabledRequest,
  AgentTemplateDeleteRequest,
  AgentTemplateProjectAssignmentRequest
} from './types'
import {
  record,
  exact,
  MAX_TEXT_BYTES,
  text,
  utf8LessThan,
  schema,
  nullableText,
  bool,
  integer
} from './validation'

export function parseAgentTemplate(value: unknown): AgentTemplate {
  const item = record(value, 'AgentTemplate')
  exact(
    item,
    [
      'schemaVersion',
      'templateId',
      'projectIds',
      'machineKey',
      'name',
      'description',
      'instructions',
      'modelConfigId',
      'modelDisplayName',
      'enabled',
      'revision',
      'createdAt',
      'updatedAt'
    ],
    'AgentTemplate'
  )
  if (typeof item.description !== 'string' || typeof item.instructions !== 'string')
    throw new Error('Invalid template text')
  if (
    new TextEncoder().encode(item.description).byteLength > MAX_TEXT_BYTES ||
    new TextEncoder().encode(item.instructions).byteLength > MAX_TEXT_BYTES
  )
    throw new Error('Invalid template text')
  if (!Array.isArray(item.projectIds) || item.projectIds.length > 256) {
    throw new Error('Invalid AgentTemplate.projectIds')
  }
  const projectIds = item.projectIds.map((projectId, index) =>
    text(projectId, `AgentTemplate.projectIds[${index}]`)
  )
  if (
    projectIds.some(
      (projectId, index) => index > 0 && !utf8LessThan(projectIds[index - 1]!, projectId)
    )
  ) {
    throw new Error('Invalid AgentTemplate.projectIds')
  }
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTemplate'),
    templateId: text(item.templateId, 'templateId'),
    projectIds,
    machineKey: text(item.machineKey, 'machineKey'),
    name: text(item.name, 'name'),
    description: item.description,
    instructions: item.instructions,
    modelConfigId: text(item.modelConfigId, 'modelConfigId', 1_024),
    modelDisplayName: nullableText(item.modelDisplayName, 'modelDisplayName'),
    enabled: bool(item.enabled, 'enabled'),
    revision: integer(item.revision, 'revision', 1),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseAgentTemplateList(value: unknown): AgentTemplateList {
  const item = record(value, 'AgentTemplateList')
  exact(item, ['schemaVersion', 'templates'], 'AgentTemplateList')
  if (!Array.isArray(item.templates) || item.templates.length > 256)
    throw new Error('Invalid templates')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTemplateList'),
    templates: item.templates.map(parseAgentTemplate)
  }
}

export function parseAgentTemplateListRequest(value: unknown): AgentTemplateListRequest {
  const item = record(value, 'AgentTemplateListRequest')
  exact(item, ['includeDisabled'], 'AgentTemplateListRequest')
  return {
    includeDisabled: bool(item.includeDisabled, 'includeDisabled')
  }
}

export function parseAgentTemplateCreateRequest(value: unknown): AgentTemplateCreateRequest {
  const item = record(value, 'AgentTemplateCreateRequest')
  exact(
    item,
    ['templateId', 'machineKey', 'name', 'description', 'instructions', 'modelConfigId', 'enabled'],
    'AgentTemplateCreateRequest'
  )
  const description = typeof item.description === 'string' ? item.description : null
  const instructions = typeof item.instructions === 'string' ? item.instructions : null
  if (
    description === null ||
    instructions === null ||
    new TextEncoder().encode(description).byteLength > MAX_TEXT_BYTES ||
    new TextEncoder().encode(instructions).byteLength > MAX_TEXT_BYTES
  ) {
    throw new Error('Invalid AgentTemplateCreateRequest text')
  }
  return {
    templateId: text(item.templateId, 'templateId'),
    machineKey: text(item.machineKey, 'machineKey'),
    name: text(item.name, 'name'),
    description,
    instructions,
    modelConfigId: text(item.modelConfigId, 'modelConfigId', 1_024),
    enabled: bool(item.enabled, 'enabled')
  }
}

export function parseAgentTemplateUpdateRequest(value: unknown): AgentTemplateUpdateRequest {
  const item = record(value, 'AgentTemplateUpdateRequest')
  exact(
    item,
    ['templateId', 'expectedRevision', 'name', 'description', 'instructions', 'modelConfigId'],
    'AgentTemplateUpdateRequest'
  )
  const common = parseAgentTemplateCreateRequest({
    templateId: item.templateId,
    machineKey: 'immutable-machine-key',
    name: item.name,
    description: item.description,
    instructions: item.instructions,
    modelConfigId: item.modelConfigId,
    enabled: true
  })
  return {
    templateId: common.templateId,
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1),
    name: common.name,
    description: common.description,
    instructions: common.instructions,
    modelConfigId: common.modelConfigId
  }
}

export function parseAgentTemplateSetEnabledRequest(
  value: unknown
): AgentTemplateSetEnabledRequest {
  const item = record(value, 'AgentTemplateSetEnabledRequest')
  exact(item, ['templateId', 'expectedRevision', 'enabled'], 'AgentTemplateSetEnabledRequest')
  return {
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1),
    enabled: bool(item.enabled, 'enabled')
  }
}

export function parseAgentTemplateDeleteRequest(value: unknown): AgentTemplateDeleteRequest {
  const item = record(value, 'AgentTemplateDeleteRequest')
  exact(item, ['templateId', 'expectedRevision'], 'AgentTemplateDeleteRequest')
  return {
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1)
  }
}

export function parseAgentTemplateProjectAssignmentRequest(
  value: unknown
): AgentTemplateProjectAssignmentRequest {
  const item = record(value, 'AgentTemplateProjectAssignmentRequest')
  exact(item, ['projectId', 'templateId', 'assigned'], 'AgentTemplateProjectAssignmentRequest')
  return {
    projectId: text(item.projectId, 'projectId'),
    templateId: text(item.templateId, 'templateId'),
    assigned: bool(item.assigned, 'assigned')
  }
}
