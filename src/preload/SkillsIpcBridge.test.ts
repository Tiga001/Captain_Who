import type { IpcRenderer } from 'electron'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  SkillInstallationPreview,
  SkillMutationOutput,
  SkillsChangedNotification,
  SkillsCommitInstallationInput,
  SkillsInspectInstallationInput,
  SkillsListOutput,
  SkillsSetEnabledInput,
  SkillsUninstallInput
} from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { createSkillsIpcBridge } from './SkillsIpcBridge'

type SkillsIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

function createIpcRenderer(): {
  invoke: ReturnType<typeof vi.fn>
  on: ReturnType<typeof vi.fn>
  removeListener: ReturnType<typeof vi.fn>
  renderer: SkillsIpcRenderer
} {
  const invoke = vi.fn()
  const on = vi.fn()
  const removeListener = vi.fn()
  return {
    invoke,
    on,
    removeListener,
    renderer: { invoke, on, removeListener } as unknown as SkillsIpcRenderer
  }
}

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
    const ipc = createIpcRenderer()
    ipc.invoke.mockResolvedValue(catalog)
    const bridge = createSkillsIpcBridge(ipc.renderer)

    const result = await bridge.list({ projectId: 'project-1' })

    expect(ipc.invoke).toHaveBeenCalledWith('host:skills.list', { projectId: 'project-1' })
    expect(result).toBe(catalog)
    expect(result.skills[0]?.source).toEqual({
      id: 'installed:user',
      kind: 'installed'
    })
  })

  it('routes the installation workflow without unwrapping structured invocation results', async () => {
    const inspectInput = {
      preparationId: 'preparation-1',
      intent: { operation: 'install' },
      source: { kind: 'localDirectory', directory: '/tmp/repository-auditor' }
    } satisfies SkillsInspectInstallationInput
    const commitInput = {
      preparationId: 'preparation-1',
      previewRevision: 'preview-revision-1',
      acceptedIssueIds: []
    } satisfies SkillsCommitInstallationInput
    const setEnabledInput = {
      skillId: 'installed:user:skill-1',
      expectedStateRevision: 'state-revision-1',
      enabled: false
    } satisfies SkillsSetEnabledInput
    const response = {
      ok: false,
      error: {
        message: 'The source is temporarily unavailable.',
        code: -32011,
        data: { type: 'skillInspection', code: 'networkUnavailable' }
      }
    } satisfies HostInvocationResult<SkillInstallationPreview>
    const ipc = createIpcRenderer()
    ipc.invoke.mockResolvedValue(response)
    const bridge = createSkillsIpcBridge(ipc.renderer)

    await expect(bridge.inspectInstallation(inspectInput)).resolves.toBe(response)
    await bridge.commitInstallation(commitInput)
    await bridge.cancelPreparation({ preparationId: 'preparation-1' })
    await bridge.listManagement({})
    await bridge.setEnabled(setEnabledInput)

    expect(ipc.invoke.mock.calls).toEqual([
      ['host:skills.inspectInstallation', inspectInput],
      ['host:skills.commitInstallation', commitInput],
      ['host:skills.cancelPreparation', { preparationId: 'preparation-1' }],
      ['host:skills.listManagement', {}],
      ['host:skills.setEnabled', setEnabledInput]
    ])
  })

  it('routes native directory selection and managed uninstall across the transport boundary', async () => {
    const uninstallInput = {
      skillId: 'installed:user:skill-1',
      expectedRevision: 'skill-package-sha256-v1:current'
    } satisfies SkillsUninstallInput
    const uninstallResponse = {
      ok: false,
      error: {
        message: 'The installed Skill changed. Refresh and try again.',
        code: -32010,
        data: {
          type: 'skillInstallation',
          operation: 'uninstall',
          code: 'revisionConflict',
          recovery: 'refreshCatalog',
          message: 'The installed Skill changed. Refresh and try again.',
          commitMayHaveSucceeded: false,
          skillId: uninstallInput.skillId,
          expectedRevision: uninstallInput.expectedRevision,
          actualRevision: 'skill-package-sha256-v1:new'
        }
      }
    } satisfies HostInvocationResult<SkillMutationOutput>
    const ipc = createIpcRenderer()
    ipc.invoke.mockResolvedValueOnce('/tmp/local-skill').mockResolvedValueOnce(uninstallResponse)
    const bridge = createSkillsIpcBridge(ipc.renderer)

    await expect(bridge.selectInstallationDirectory()).resolves.toBe('/tmp/local-skill')
    await expect(bridge.uninstall(uninstallInput)).resolves.toBe(uninstallResponse)

    expect(ipc.invoke.mock.calls).toEqual([
      ['host:skills.selectInstallationDirectory'],
      ['host:skills.uninstall', uninstallInput]
    ])
  })

  it('routes changed notifications and removes the exact registered listener', () => {
    const changed = {
      schemaVersion: 1,
      managementRevision: 'management-revision-2',
      reason: 'enablementChanged',
      skillId: 'installed:user:skill-1'
    } satisfies SkillsChangedNotification
    const ipc = createIpcRenderer()
    const bridge = createSkillsIpcBridge(ipc.renderer)
    const handler = vi.fn()

    const unsubscribe = bridge.onChanged(handler)
    const listener = ipc.on.mock.calls[0]?.[1] as
      ((event: unknown, payload: SkillsChangedNotification) => void) | undefined
    expect(ipc.on).toHaveBeenCalledWith('host:skills.changed', expect.any(Function))

    listener?.({}, changed)
    expect(handler).toHaveBeenCalledWith(changed)

    unsubscribe()
    expect(ipc.removeListener).toHaveBeenCalledWith('host:skills.changed', listener)
  })
})
