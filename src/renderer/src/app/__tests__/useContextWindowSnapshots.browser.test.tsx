import type {
  AgentContextWindowSnapshot,
  AgentContextWindowSnapshotOutput,
  AgentPermissions,
  SkillSelection
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ChatPermissionMode } from '../../features/chat/chatTypes'

const agentClient = vi.hoisted(() => ({
  getContextWindowSnapshot: vi.fn(),
  settingsListener: null as
    null | ((settings: { enabled: boolean; revision: number; updatedAt: number }) => void)
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onCollaborationSettingsChanged: (listener: typeof agentClient.settingsListener) => {
        agentClient.settingsListener = listener
        return () => {
          agentClient.settingsListener = null
        }
      }
    }
  }
}))

vi.mock('../../features/agent/agentClient', () => ({
  getContextWindowSnapshot: agentClient.getContextWindowSnapshot
}))

const { useContextWindowSnapshots } =
  await import('../../features/agentRun/useContextWindowSnapshots')

const CUSTOM_PERMISSIONS: AgentPermissions = {
  read: 'workspace_only',
  write: 'workspace_only',
  command: 'require_approval',
  commandSafety: 'guarded',
  patch: 'require_approval',
  builtinExecution: 'require_approval'
}

interface HarnessProps {
  customPermissions?: AgentPermissions
  eventModelConfigId?: string
  eventSnapshot?: AgentContextWindowSnapshot
  isRunning?: boolean
  modelId?: string
  permissionMode?: ChatPermissionMode
  projectId?: string
  skills: SkillSelection[]
}

function Harness({
  customPermissions = CUSTOM_PERMISSIONS,
  eventModelConfigId = 'model-1',
  eventSnapshot,
  isRunning = false,
  modelId = 'model-1',
  permissionMode = 'default',
  projectId = 'project-1',
  skills
}: HarnessProps) {
  const { activeSnapshot, recordSnapshot } = useContextWindowSnapshots({
    conversationId: 'conversation-1',
    customPermissions,
    enabled: true,
    isRunning,
    modelId,
    permissionMode,
    projectId,
    scopeId: 'conversation-1',
    skills
  })

  return (
    <div>
      <output data-testid="input-tokens">{activeSnapshot?.inputTokens ?? 'none'}</output>
      {eventSnapshot ? (
        <button
          onClick={() => recordSnapshot('conversation-1', eventModelConfigId, eventSnapshot)}
          type="button"
        >
          record event
        </button>
      ) : null}
    </div>
  )
}

function snapshot(inputTokens: number, model = 'provider-model-1'): AgentContextWindowSnapshot {
  return {
    model,
    status: 'within_budget',
    contextWindowTokens: 256_000,
    reservedOutputTokens: 30_000,
    safetyMarginTokens: 12_800,
    inputCapacityTokens: 213_200,
    inputTokens,
    costBreakdown: {
      systemTokens: 10_000,
      toolSchemaTokens: 5_000,
      summaryTokens: 0,
      worldStateTokens: 0,
      todoTokens: 0,
      providerContinuationTokens: 0,
      recentHistoryTokens: Math.max(0, inputTokens - 15_000),
      totalInputTokens: inputTokens
    },
    remainingInputTokens: 213_200 - inputTokens
  }
}

function deferred<Value>() {
  let resolve!: (value: Value) => void
  const promise = new Promise<Value>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}

describe('useContextWindowSnapshots', () => {
  beforeEach(() => {
    agentClient.getContextWindowSnapshot.mockReset().mockResolvedValue({ modelConfigId: 'model-1' })
  })

  it('refreshes the ring when global collaboration settings change', async () => {
    agentClient.getContextWindowSnapshot
      .mockResolvedValueOnce({ modelConfigId: 'model-1', snapshot: snapshot(21000) })
      .mockResolvedValueOnce({ modelConfigId: 'model-1', snapshot: snapshot(19000) })
    const screen = await render(<Harness skills={[]} />)
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('21000')
    agentClient.settingsListener?.({ enabled: false, revision: 2, updatedAt: 1 })
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('19000')
    expect(agentClient.getContextWindowSnapshot).toHaveBeenCalledTimes(2)
  })

  it('indexes an inspection by Host-owned modelConfigId while retaining the provider wire model', async () => {
    agentClient.getContextWindowSnapshot.mockResolvedValueOnce({
      modelConfigId: 'model-1',
      snapshot: snapshot(21_000, 'provider-wire-model')
    })

    const screen = await render(<Harness skills={[]} />)

    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('21000')
  })

  it('does not repeat the RPC when equivalent skills and permissions get new references', async () => {
    const screen = await render(
      <Harness
        customPermissions={{ ...CUSTOM_PERMISSIONS }}
        skills={[{ id: 'installed:spreadsheets', revision: 'revision-1' }]}
      />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(1)

    await screen.rerender(
      <Harness
        customPermissions={{ ...CUSTOM_PERMISSIONS }}
        skills={[{ id: 'installed:spreadsheets', revision: 'revision-1' }]}
      />
    )
    await Promise.resolve()

    expect(agentClient.getContextWindowSnapshot).toHaveBeenCalledTimes(1)
  })

  it('refreshes when a skill, project, resolved permission, or model really changes', async () => {
    const screen = await render(
      <Harness skills={[{ id: 'installed:spreadsheets', revision: 'revision-1' }]} />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(1)

    await screen.rerender(
      <Harness skills={[{ id: 'installed:spreadsheets', revision: 'revision-2' }]} />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(2)

    await screen.rerender(
      <Harness
        projectId="project-2"
        skills={[{ id: 'installed:spreadsheets', revision: 'revision-2' }]}
      />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(3)

    await screen.rerender(
      <Harness
        permissionMode="full"
        projectId="project-2"
        skills={[{ id: 'installed:spreadsheets', revision: 'revision-2' }]}
      />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(4)

    await screen.rerender(
      <Harness
        modelId="model-2"
        permissionMode="full"
        projectId="project-2"
        skills={[{ id: 'installed:spreadsheets', revision: 'revision-2' }]}
      />
    )
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(5)

    expect(agentClient.getContextWindowSnapshot.mock.calls.at(-1)?.[0]).toMatchObject({
      conversationId: 'conversation-1',
      modelId: 'model-2',
      projectId: 'project-2',
      skills: [{ id: 'installed:spreadsheets', revision: 'revision-2' }],
      permissions: {
        read: 'all',
        write: 'all',
        command: 'auto_approve',
        commandSafety: 'full_access',
        patch: 'auto_approve',
        builtinExecution: 'auto_approve'
      }
    })
  })

  it('keeps a running event snapshot when the older inspection request completes later', async () => {
    const inspection = deferred<AgentContextWindowSnapshotOutput>()
    agentClient.getContextWindowSnapshot.mockReturnValueOnce(inspection.promise)
    const eventSnapshot = snapshot(42_000, 'shared-provider-wire-model')
    const screen = await render(<Harness eventSnapshot={eventSnapshot} skills={[]} />)
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(1)

    await screen.getByRole('button', { name: 'record event' }).click()
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('42000')

    inspection.resolve({
      modelConfigId: 'model-1',
      snapshot: snapshot(10_000, 'shared-provider-wire-model')
    })
    await inspection.promise
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('42000')
  })

  it('uses Host usage throughout a Run and previews the next configuration only after it ends', async () => {
    const inspection = deferred<AgentContextWindowSnapshotOutput>()
    agentClient.getContextWindowSnapshot.mockReturnValueOnce(inspection.promise)
    const screen = await render(<Harness skills={[]} />)
    await expect.poll(() => agentClient.getContextWindowSnapshot.mock.calls.length).toBe(1)

    await screen.rerender(<Harness isRunning eventSnapshot={snapshot(42_000)} skills={[]} />)
    await screen.getByRole('button', { name: 'record event' }).click()

    // Model selection is pinned by AppShell to the active Run. Future permissions/skills remain
    // editable in the composer even while a command executes or an approval is pending.
    await screen.rerender(
      <Harness
        isRunning
        permissionMode="full"
        eventSnapshot={snapshot(43_000)}
        skills={[{ id: 'installed:spreadsheets', revision: 'next-revision' }]}
      />
    )
    agentClient.settingsListener?.({ enabled: true, revision: 2, updatedAt: 1 })
    inspection.resolve({ modelConfigId: 'model-1', snapshot: snapshot(10_000) })
    await inspection.promise
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('42000')
    expect(agentClient.getContextWindowSnapshot).toHaveBeenCalledTimes(1)

    await screen.getByRole('button', { name: 'record event' }).click()
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('43000')

    agentClient.getContextWindowSnapshot.mockResolvedValueOnce({
      modelConfigId: 'model-2',
      snapshot: snapshot(18_000, 'provider-model-2')
    })
    await screen.rerender(
      <Harness
        modelId="model-2"
        permissionMode="full"
        skills={[{ id: 'installed:spreadsheets', revision: 'next-revision' }]}
      />
    )
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('18000')
    expect(agentClient.getContextWindowSnapshot).toHaveBeenCalledTimes(2)
    expect(agentClient.getContextWindowSnapshot).toHaveBeenLastCalledWith(
      expect.objectContaining({
        modelId: 'model-2',
        permissions: expect.objectContaining({
          command: 'auto_approve',
          commandSafety: 'full_access'
        }),
        skills: [{ id: 'installed:spreadsheets', revision: 'next-revision' }]
      })
    )
  })

  it('can observe an already-running conversation without estimating from its future draft', async () => {
    const screen = await render(
      <Harness isRunning permissionMode="full" eventSnapshot={snapshot(25_000)} skills={[]} />
    )
    await screen.getByRole('button', { name: 'record event' }).click()
    await expect.element(screen.getByTestId('input-tokens')).toHaveTextContent('25000')
    expect(agentClient.getContextWindowSnapshot).not.toHaveBeenCalled()
  })
})
