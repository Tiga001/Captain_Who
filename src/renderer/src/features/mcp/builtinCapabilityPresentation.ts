import type { McpBuiltinCapabilityId } from '@mycopilot/protocol'
import type { TranslationKey } from '../../config/frontendTranslations'
import type { Translate } from '../../config/translationFormat'

const capabilityNameKeys = {
  browser_automation: 'mcp.builtin.browserAutomation.name'
} as const satisfies Record<McpBuiltinCapabilityId, TranslationKey>

/** Renderer-owned presentation for Host-registered built-in capability identities. */
export function getBuiltinCapabilityDisplayName(
  capabilityId: McpBuiltinCapabilityId,
  t: Translate
): string {
  return t(capabilityNameKeys[capabilityId])
}
