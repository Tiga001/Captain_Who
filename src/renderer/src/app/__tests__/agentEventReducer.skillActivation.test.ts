import { describe, expect, it } from 'vitest'
import type { ChatMessage } from '../../features/chat/chatTypes'
import { applyAgentEventToChatMessage } from '../agentEventReducer'

describe('Skill activation runtime event compatibility', () => {
  it('does not treat a non-terminal activation notification as run completion', () => {
    const message: ChatMessage = {
      id: 'assistant-message',
      role: 'assistant',
      content: '',
      createdAt: 1,
      status: 'pending'
    }

    const result = applyAgentEventToChatMessage(message, {
      type: 'skill_activated',
      runId: 'run-skill-activation',
      skill: {
        id: 'bundled:application:documents',
        name: 'documents',
        revision: 'skill-package-sha256-v2:test',
        source: 'bundled:application',
        activatedBy: 'model'
      }
    })

    expect(result).toBe(message)
    expect(result.status).toBe('pending')
  })
})
