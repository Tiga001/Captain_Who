import { SKILL_CATALOG_SCHEMA_VERSION, type SkillsListOutput } from '@mycopilot/protocol'

/**
 * Reject catalog capabilities this client does not understand. A source's trust classification is
 * display and policy metadata; it never grants permissions to the activated instructions.
 */
export function assertSupportedSkillCatalog(output: SkillsListOutput): void {
  if (output.schemaVersion !== SKILL_CATALOG_SCHEMA_VERSION) {
    throw new Error(`Unsupported Skill catalog schema: ${String(output.schemaVersion)}`)
  }

  for (const descriptor of output.skills) {
    const sourceKind: unknown = descriptor.source.kind
    const trust: unknown = descriptor.trust
    const supportedCombination =
      (sourceKind === 'workspace' && trust === 'untrusted') ||
      (sourceKind === 'bundled' && trust === 'application')

    if (!supportedCombination || descriptor.activationScope !== 'run') {
      throw new Error(
        `Unsupported Skill source contract for ${descriptor.id}: ${String(sourceKind)}/${String(trust)}/${String(descriptor.activationScope)}`
      )
    }
  }
}
