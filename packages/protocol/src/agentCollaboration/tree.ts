import type {
  AgentTreeRequest,
  AgentDetailRequest,
  AgentModelDisplay,
  AgentSummary,
  AgentTreeSnapshot,
  AgentTreeLookup,
  AgentDetail,
  AgentTemplateBinding,
  AgentConversationLocator
} from './types'
import { record, exact, text, nullableText, oneOf, integer, schema, bool } from './validation'

export function parseAgentTreeRequest(value: unknown): AgentTreeRequest {
  const item = record(value, 'AgentTreeRequest')
  exact(item, ['rootConversationId'], 'AgentTreeRequest')
  return { rootConversationId: text(item.rootConversationId, 'rootConversationId') }
}

export function parseAgentDetailRequest(value: unknown): AgentDetailRequest {
  const item = record(value, 'AgentDetailRequest')
  exact(item, ['rootConversationId', 'agentId'], 'AgentDetailRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId')
  }
}

export const parseAgentConversationLocatorRequest = parseAgentDetailRequest

function parseModel(value: unknown, context: string): AgentModelDisplay | null {
  if (value === null) return null
  const item = record(value, context)
  exact(item, ['modelConfigId', 'displayName'], context)
  return {
    modelConfigId: text(item.modelConfigId, `${context}.modelConfigId`),
    displayName: text(item.displayName, `${context}.displayName`)
  }
}

export function parseAgentSummary(value: unknown, context = 'AgentSummary'): AgentSummary {
  const item = record(value, context)
  exact(
    item,
    [
      'agentId',
      'rootAgentId',
      'rootConversationId',
      'parentAgentId',
      'conversationId',
      'projectId',
      'taskName',
      'taskPath',
      'lifecycle',
      'displayStatus',
      'latestActivityAt',
      'model'
    ],
    context
  )
  return {
    agentId: text(item.agentId, `${context}.agentId`),
    rootAgentId: text(item.rootAgentId, `${context}.rootAgentId`),
    rootConversationId: text(item.rootConversationId, `${context}.rootConversationId`),
    parentAgentId: nullableText(item.parentAgentId, `${context}.parentAgentId`),
    conversationId: text(item.conversationId, `${context}.conversationId`),
    projectId: nullableText(item.projectId, `${context}.projectId`),
    taskName: text(item.taskName, `${context}.taskName`),
    taskPath: text(item.taskPath, `${context}.taskPath`, 4_096),
    lifecycle: oneOf(
      item.lifecycle,
      ['active', 'archived', 'disabled'] as const,
      `${context}.lifecycle`
    ),
    displayStatus: oneOf(
      item.displayStatus,
      [
        'idle',
        'queued',
        'running',
        'waiting_approval',
        'latest_completed',
        'latest_failed',
        'latest_interrupted',
        'latest_outcome_unknown',
        'archived',
        'disabled'
      ] as const,
      `${context}.displayStatus`
    ),
    latestActivityAt: integer(item.latestActivityAt, `${context}.latestActivityAt`),
    model: parseModel(item.model, `${context}.model`)
  }
}

export function parseAgentTreeSnapshot(value: unknown): AgentTreeSnapshot {
  const item = record(value, 'AgentTreeSnapshot')
  exact(
    item,
    [
      'schemaVersion',
      'workspaceId',
      'projectId',
      'rootAgentId',
      'rootConversationId',
      'agents',
      'lastSequence'
    ],
    'AgentTreeSnapshot'
  )
  if (!Array.isArray(item.agents) || item.agents.length > 1_024)
    throw new Error('Invalid AgentTreeSnapshot.agents')
  const parsed: AgentTreeSnapshot = {
    schemaVersion: schema(item.schemaVersion, 'AgentTreeSnapshot'),
    workspaceId: nullableText(item.workspaceId, 'workspaceId'),
    projectId: nullableText(item.projectId, 'projectId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agents: item.agents.map((entry, index) => parseAgentSummary(entry, `agents[${index}]`)),
    lastSequence: integer(item.lastSequence, 'lastSequence')
  }
  const agentsById = new Map<string, AgentSummary>()
  const conversationIds = new Set<string>()
  const taskPaths = new Set<string>()
  for (const agent of parsed.agents) {
    if (
      agentsById.has(agent.agentId) ||
      conversationIds.has(agent.conversationId) ||
      taskPaths.has(agent.taskPath)
    ) {
      throw new Error('Invalid AgentTreeSnapshot duplicate identity')
    }
    agentsById.set(agent.agentId, agent)
    conversationIds.add(agent.conversationId)
    taskPaths.add(agent.taskPath)
  }
  const root = agentsById.get(parsed.rootAgentId)
  if (
    parsed.workspaceId !== parsed.projectId ||
    root === undefined ||
    root.parentAgentId !== null ||
    root.conversationId !== parsed.rootConversationId ||
    parsed.agents.some(
      (agent) =>
        agent.rootAgentId !== parsed.rootAgentId ||
        agent.rootConversationId !== parsed.rootConversationId ||
        agent.projectId !== parsed.projectId
    )
  ) {
    throw new Error('Invalid AgentTreeSnapshot identity')
  }
  for (const agent of parsed.agents) {
    const visited = new Set<string>()
    let current: AgentSummary | undefined = agent
    while (current.agentId !== parsed.rootAgentId) {
      if (visited.has(current.agentId) || current.parentAgentId === null) {
        throw new Error('Invalid AgentTreeSnapshot parent relation')
      }
      visited.add(current.agentId)
      current = agentsById.get(current.parentAgentId)
      if (current === undefined) throw new Error('Invalid AgentTreeSnapshot parent relation')
    }
  }
  return parsed
}

export function parseAgentTreeLookup(value: unknown): AgentTreeLookup {
  const item = record(value, 'AgentTreeLookup')
  exact(item, ['schemaVersion', 'materialized', 'tree'], 'AgentTreeLookup')
  const materialized = bool(item.materialized, 'materialized')
  const tree = item.tree === null ? null : parseAgentTreeSnapshot(item.tree)
  if (materialized !== (tree !== null)) throw new Error('Invalid AgentTreeLookup state')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTreeLookup'),
    materialized,
    tree
  }
}

export function parseAgentDetail(value: unknown): AgentDetail {
  const item = record(value, 'AgentDetail')
  exact(
    item,
    [
      'schemaVersion',
      'summary',
      'template',
      'reasoningEffort',
      'revision',
      'createdAt',
      'updatedAt'
    ],
    'AgentDetail'
  )
  let template: AgentTemplateBinding | null = null
  if (item.template !== null) {
    const binding = record(item.template, 'AgentDetail.template')
    exact(
      binding,
      ['templateId', 'machineKey', 'name', 'description', 'revision'],
      'AgentDetail.template'
    )
    template = {
      templateId: text(binding.templateId, 'templateId'),
      machineKey: text(binding.machineKey, 'machineKey'),
      name: text(binding.name, 'name'),
      description:
        typeof binding.description === 'string'
          ? binding.description
          : (() => {
              throw new Error('Invalid description')
            })(),
      revision: integer(binding.revision, 'revision', 1)
    }
  }
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentDetail'),
    summary: parseAgentSummary(item.summary),
    template,
    reasoningEffort: nullableText(item.reasoningEffort, 'reasoningEffort'),
    revision: integer(item.revision, 'revision', 1),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseAgentConversationLocator(value: unknown): AgentConversationLocator {
  const item = record(value, 'AgentConversationLocator')
  exact(item, ['schemaVersion', 'agentId', 'conversationId', 'mode'], 'AgentConversationLocator')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentConversationLocator'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    mode: oneOf(item.mode, ['interactive', 'observer'] as const, 'mode')
  }
}
