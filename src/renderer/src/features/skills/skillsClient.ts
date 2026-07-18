import type { SkillsListOutput } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { assertSupportedSkillCatalog } from './skillCatalog'

export async function listSkills(projectId: string): Promise<SkillsListOutput> {
  const output = await hostClient.skills.list({ projectId })
  assertSupportedSkillCatalog(output)
  return output
}
