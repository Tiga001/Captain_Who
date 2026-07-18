import type { IpcRenderer } from 'electron'
import type { SkillsListOutput } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createSkillsIpcBridge } from './SkillsIpcBridge'

describe('Skills IPC bridge', () => {
  it('round-trips a schema-v4 installed catalog without changing opaque identities', async () => {
    const catalog = {
      schemaVersion: 4,
      catalogRevision: 'catalog-revision',
      diagnostics: [],
      skills: [
        {
          activationScope: 'run',
          description: 'Audit repository claims using evidence.',
          id: 'installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d',
          location:
            'packages/v1/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/SKILL.md',
          name: 'installed-auditor',
          revision:
            'skill-package-sha256-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
          source: { id: 'installed:user', kind: 'installed' },
          trust: 'untrusted'
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
      id: 'installed:user',
      kind: 'installed'
    })
  })
})
