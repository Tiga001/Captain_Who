import type { AgentContextProfile } from './agent'

export const AGENT_PROMPT_PREFERENCES_CHANGED_METHOD = 'agent.promptPreferencesChanged'

/** Invalidation metadata only; custom instructions never enter notification payloads. */
export interface AgentPromptPreferencesChanged {
  contextProfile: AgentContextProfile
  updatedAt: number
}

export function parseAgentPromptPreferencesChanged(value: unknown): AgentPromptPreferencesChanged {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid AgentPromptPreferencesChanged')
  }
  const record = value as Record<string, unknown>
  if (
    Object.keys(record).some((key) => key !== 'contextProfile' && key !== 'updatedAt') ||
    (record.contextProfile !== 'full' && record.contextProfile !== 'minimal') ||
    typeof record.updatedAt !== 'number' ||
    !Number.isSafeInteger(record.updatedAt) ||
    record.updatedAt < 0
  ) {
    throw new Error('Invalid AgentPromptPreferencesChanged')
  }
  return { contextProfile: record.contextProfile, updatedAt: record.updatedAt }
}
