import type { IpcRenderer } from 'electron'
import type { SkillsListOutput } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createSkillsIpcBridge } from './SkillsIpcBridge'

describe('Skills IPC bridge', () => {
  it('round-trips a schema-v3 dual-source catalog without changing opaque identities', async () => {
    const catalog = {
      schemaVersion: 3,
      catalogRevision: 'catalog-revision',
      diagnostics: [],
      skills: [
        {
          activationScope: 'run',
          description: 'Audit repository claims using evidence.',
          id: 'bundled:application:repository-evidence-auditor',
          location: 'repository-evidence-auditor/SKILL.md',
          name: 'repository-evidence-auditor',
          revision: 'skill-sha256-v1:revision',
          source: { id: 'bundled:application', kind: 'bundled' },
          trust: 'application'
        }
      ],
      truncated: false
    } satisfies SkillsListOutput
    const invoke = vi.fn().mockResolvedValue(catalog)
    const bridge = createSkillsIpcBridge({
      invoke
    } as unknown as Pick<IpcRenderer, 'invoke'>)

    const result = await bridge.list({ projectId: 'project-1' })

    expect(invoke).toHaveBeenCalledWith('host:skills.list', { projectId: 'project-1' })
    expect(result).toBe(catalog)
    expect(result.skills[0]?.source).toEqual({
      id: 'bundled:application',
      kind: 'bundled'
    })
  })
})
