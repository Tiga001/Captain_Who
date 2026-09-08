import { describe, expect, it } from 'vitest'
import {
  AGENT_PROMPT_PREFERENCES_CHANGED_METHOD,
  parseAgentPromptPreferencesChanged
} from './agentPromptPreferences'

describe('prompt preference invalidation metadata', () => {
  it('accepts both profiles without carrying custom instructions', () => {
    expect(AGENT_PROMPT_PREFERENCES_CHANGED_METHOD).toBe('agent.promptPreferencesChanged')
    for (const contextProfile of ['full', 'minimal']) {
      expect(parseAgentPromptPreferencesChanged({ contextProfile, updatedAt: 12 })).toEqual({
        contextProfile,
        updatedAt: 12
      })
    }
  })

  it('rejects unknown profiles, invalid timestamps and unexpected payloads', () => {
    for (const payload of [
      null,
      {},
      { contextProfile: 'tiny', updatedAt: 1 },
      { contextProfile: 'full', updatedAt: -1 },
      { contextProfile: 'full', updatedAt: 0.5 },
      { contextProfile: 'full', updatedAt: 0, customInstructions: 'private' }
    ])
      expect(() => parseAgentPromptPreferencesChanged(payload)).toThrow()
  })
})
