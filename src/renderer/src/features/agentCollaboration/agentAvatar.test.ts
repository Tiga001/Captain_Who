import { describe, expect, it } from 'vitest'
import { AGENT_AVATAR_URLS } from '../../assets/agent-avatars/manifest'
import {
  AGENT_AVATAR_COUNT,
  getAgentAvatarIndex,
  getAgentAvatarUrl,
  validateAgentAvatarManifest
} from './agentAvatarAssignment'

describe('Agent avatar assignment', () => {
  it('maps immutable Agent ids deterministically across all entry points and reloads', () => {
    expect(getAgentAvatarIndex('agent-a')).toBe(57)
    expect(getAgentAvatarIndex('agent-b')).toBe(1)
    expect(getAgentAvatarIndex('agent-你好')).toBe(15)

    const firstProjection = Array.from({ length: 256 }, (_, index) =>
      getAgentAvatarUrl(`agent-${index}`)
    )
    const reloadedProjection = Array.from({ length: 256 }, (_, index) =>
      getAgentAvatarUrl(`agent-${index}`)
    )
    expect(reloadedProjection).toEqual(firstProjection)
  })

  it('keeps collisions bounded and deterministic without using names or models as selectors', () => {
    const assignments = new Map<number, string>()
    let collision: [string, string] | undefined
    for (let index = 0; index <= AGENT_AVATAR_COUNT; index += 1) {
      const agentId = `collision-agent-${index}`
      const avatarIndex = getAgentAvatarIndex(agentId)
      const previous = assignments.get(avatarIndex)
      if (previous) {
        collision = [previous, agentId]
        break
      }
      assignments.set(avatarIndex, agentId)
    }

    expect(collision).toBeDefined()
    const [firstId, secondId] = collision!
    expect(firstId).not.toBe(secondId)
    expect(getAgentAvatarIndex(firstId)).toBe(getAgentAvatarIndex(secondId))
    expect(getAgentAvatarUrl(firstId)).toBe(getAgentAvatarUrl(secondId))
    expect(getAgentAvatarIndex(firstId)).toBeGreaterThanOrEqual(0)
    expect(getAgentAvatarIndex(firstId)).toBeLessThan(AGENT_AVATAR_COUNT)
  })

  it('ships exactly 96 non-remote local assets', () => {
    expect(validateAgentAvatarManifest()).toBe(true)
    expect(AGENT_AVATAR_URLS).toHaveLength(AGENT_AVATAR_COUNT)
    expect(new Set(AGENT_AVATAR_URLS).size).toBe(AGENT_AVATAR_COUNT)
    for (const avatarUrl of AGENT_AVATAR_URLS) {
      expect(avatarUrl).not.toMatch(/^https?:\/\//i)
      expect(avatarUrl).not.toContain('api.dicebear.com')
    }
  })
})
