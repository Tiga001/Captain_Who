import {
  AGENT_AVATAR_COUNT as BUNDLED_AGENT_AVATAR_COUNT,
  AGENT_AVATAR_URLS
} from '../../assets/agent-avatars/manifest'

const EXPECTED_AGENT_AVATAR_COUNT = 96
export const AGENT_AVATAR_COUNT = BUNDLED_AGENT_AVATAR_COUNT

/**
 * Produces an avalanche-mixed, deterministic 32-bit value from the immutable Agent id.
 *
 * The renderer deliberately owns this choice: task names, models and Provider output never
 * participate, so an Agent cannot request an avatar and renaming it cannot change its identity.
 */
function hashAgentId(agentId: string): number {
  let hash = 0x811c9dc5
  for (let index = 0; index < agentId.length; index += 1) {
    hash ^= agentId.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193)
  }

  hash ^= hash >>> 16
  hash = Math.imul(hash, 0x85ebca6b)
  hash ^= hash >>> 13
  hash = Math.imul(hash, 0xc2b2ae35)
  hash ^= hash >>> 16
  return hash >>> 0
}

export function getAgentAvatarIndex(agentId: string): number {
  return hashAgentId(agentId) % AGENT_AVATAR_COUNT
}

export function getAgentAvatarUrl(agentId: string): string {
  const avatar = AGENT_AVATAR_URLS[getAgentAvatarIndex(agentId)]
  if (!avatar) throw new Error(`Agent avatar manifest must contain ${AGENT_AVATAR_COUNT} entries`)
  return avatar
}

export function validateAgentAvatarManifest(): boolean {
  return AGENT_AVATAR_COUNT === EXPECTED_AGENT_AVATAR_COUNT && AGENT_AVATAR_URLS.every(Boolean)
}
