import type { SkillsListOutput } from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { assertSupportedSkillCatalog } from './skillCatalog'

export async function listSkills(projectId: string | null): Promise<SkillsListOutput> {
  const output = await hostClient.skills.list(projectId ? { projectId } : {})
  assertSupportedSkillCatalog(output)
  return output
}
