import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AutomationListOutput, AutomationTask } from '@mycopilot/protocol'

const automations = vi.hoisted(() => ({
  list: vi.fn(),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  setEnabled: vi.fn(),
  runNow: vi.fn(),
  delete: vi.fn(),
  listRuns: vi.fn(),
  attentionSummary: vi.fn(),
  acknowledgeAttention: vi.fn(),
  onEvent: vi.fn(() => vi.fn()),
  onResync: vi.fn(() => vi.fn())
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: { automations } }))

const { createAutomation, listAutomations, setAutomationEnabled, updateAutomation } =
  await import('../automationClient')

const task = {
  schemaVersion: 1,
  automationId: 'automation-1',
  title: 'Daily brief',
  prompt: 'Summarize.',
  status: 'active',
  health: { state: 'ok' },
  destination: {
    kind: 'new_chat',
    projectBinding: 'none',
    projectId: null,
    modelId: 'model-1',
    reasoning: { source: 'model_config', mode: 'provider_default', effort: 'provider_default' }
  },
  permissionMode: 'default',
  permissionModeVersion: 2,
  resolvedPermissions: {
    read: 'workspace_only',
    write: 'workspace_only',
    command: 'require_approval',
    commandSafety: 'guarded',
    patch: 'require_approval',
    builtinExecution: 'require_approval'
  },
  schedule: {
    kind: 'daily',
    timeMinutes: 540,
    anchorAt: 1_777_777_777_000,
    timezone: 'Asia/Shanghai'
  },
  scheduleSummary: 'Daily at 09:00',
  rrule: 'FREQ=DAILY',
  timezone: 'Asia/Shanghai',
  notificationPolicy: 'all_runs',
  targetSnapshot: { projectName: null, conversationTitle: null, modelDisplayName: 'Model 1' },
  nextRunAt: 1_777_777_778_000,
  lastScheduledAt: null,
  lastRunAt: null,
  latestRun: null,
  attention: null,
  revision: 1,
  createdAt: 1,
  updatedAt: 1
} satisfies AutomationTask

describe('automation renderer client', () => {
  beforeEach(() => {
    for (const mock of Object.values(automations)) {
      if ('mockClear' in mock) mock.mockClear()
    }
  })

  it('maps list filters to the real schema and bounds the page size', async () => {
    const output: AutomationListOutput = {
      schemaVersion: 1,
      tasks: [task],
      nextCursor: null,
      counts: { all: 1, active: 1, paused: 0 },
      attentionCount: 0,
      lastSequence: 4
    }
    automations.list.mockResolvedValue({ ok: true, value: output })
    await expect(
      listAutomations({ status: 'active', query: '  brief  ', cursor: 'next', limit: 500 })
    ).resolves.toBe(output)
    expect(automations.list).toHaveBeenCalledWith({
      schemaVersion: 1,
      status: 'active',
      query: 'brief',
      cursor: 'next',
      limit: 100
    })
  })

  it('submits only permission mode/version and captures the computer timezone on create', async () => {
    automations.create.mockResolvedValue({ ok: true, value: task })
    const staleSchedule = { ...task.schedule, timezone: 'Etc/GMT+12' }
    await createAutomation(
      {
        title: task.title,
        prompt: task.prompt,
        status: 'active',
        destination: {
          kind: 'new_chat',
          projectBinding: 'none',
          projectId: null,
          modelId: 'model-1'
        },
        permissionMode: 'default',
        permissionModeVersion: 2,
        schedule: staleSchedule,
        notificationPolicy: 'all_runs'
      },
      'request-1'
    )
    expect(automations.create).toHaveBeenCalledWith({
      schemaVersion: 1,
      requestId: 'request-1',
      title: task.title,
      prompt: task.prompt,
      status: 'active',
      destination: {
        kind: 'new_chat',
        projectBinding: 'none',
        projectId: null,
        modelId: 'model-1'
      },
      permissionMode: 'default',
      permissionModeVersion: 2,
      schedule: {
        ...staleSchedule,
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
      },
      notificationPolicy: 'all_runs'
    })
    expect(automations.create.mock.calls[0]?.[0]).not.toHaveProperty('resolvedPermissions')
  })

  it('captures the computer timezone again at the update transport boundary', async () => {
    automations.update.mockResolvedValue({ ok: true, value: task })
    const staleSchedule = { ...task.schedule, timezone: 'Etc/GMT+12' }

    await updateAutomation(
      { automationId: task.automationId, revision: task.revision },
      {
        title: task.title,
        prompt: task.prompt,
        destination: {
          kind: 'new_chat',
          projectBinding: 'none',
          projectId: null,
          modelId: 'model-1'
        },
        permissionMode: 'default',
        permissionModeVersion: 2,
        schedule: staleSchedule,
        notificationPolicy: 'all_runs'
      }
    )

    expect(automations.update).toHaveBeenCalledWith(
      expect.objectContaining({
        automationId: task.automationId,
        expectedRevision: task.revision,
        schedule: {
          ...staleSchedule,
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
        }
      })
    )
  })

  it('preserves structured revision conflict details for the UI', async () => {
    automations.setEnabled.mockResolvedValue({
      ok: false,
      error: {
        message: 'Task changed in another window.',
        code: -32045,
        data: {
          schemaVersion: 1,
          type: 'automation',
          code: 'revision_conflict',
          message: 'Task changed in another window.',
          automationId: 'automation-1',
          currentRevision: 3,
          field: null,
          retryable: true
        }
      }
    })
    await expect(
      setAutomationEnabled({ automationId: 'automation-1', revision: 1 }, false)
    ).rejects.toMatchObject({
      name: 'AutomationClientError',
      code: 'revision_conflict',
      automationId: 'automation-1',
      currentRevision: 3,
      retryable: true
    })
  })
})
