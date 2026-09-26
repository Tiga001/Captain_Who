import type {
  AgentCollaborationSettingsGetInput,
  AgentCollaborationSettings,
  AgentCollaborationSettingsUpdate
} from './types'
import { record, exact, bool, integer } from './validation'

export function parseAgentCollaborationSettingsGetInput(
  value: unknown
): AgentCollaborationSettingsGetInput {
  const item = record(value, 'AgentCollaborationSettingsGetInput')
  exact(item, [], 'AgentCollaborationSettingsGetInput')
  return {}
}

export function parseAgentCollaborationSettings(value: unknown): AgentCollaborationSettings {
  const item = record(value, 'AgentCollaborationSettings')
  exact(item, ['enabled', 'revision', 'updatedAt'], 'AgentCollaborationSettings')
  return {
    enabled: bool(item.enabled, 'enabled'),
    revision: integer(item.revision, 'revision', 1),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseAgentCollaborationSettingsUpdate(
  value: unknown
): AgentCollaborationSettingsUpdate {
  const item = record(value, 'AgentCollaborationSettingsUpdate')
  exact(item, ['enabled', 'expectedRevision'], 'AgentCollaborationSettingsUpdate')
  return {
    enabled: bool(item.enabled, 'enabled'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1)
  }
}
