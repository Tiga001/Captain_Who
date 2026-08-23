import { beforeEach, describe, expect, it } from 'vitest'
import type { AutomationTask } from '@mycopilot/protocol'
import {
  cacheAutomationTask,
  getCachedAutomationTask,
  removeCachedAutomationTask,
  resetAutomationCacheForTests
} from '../automationCache'

function task(revision: number, updatedAt = revision): AutomationTask {
  return {
    schemaVersion: 1,
    automationId: 'automation-1',
    title: `Task revision ${revision}`,
    prompt: 'Run the task.',
    status: 'active',
    health: { state: 'ok' },
    destination: { kind: 'existing_chat', conversationId: 'conversation-1' },
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
    notificationPolicy: 'important_updates',
    targetSnapshot: {
      projectName: null,
      conversationTitle: 'Conversation',
      modelDisplayName: null
    },
    nextRunAt: null,
    lastScheduledAt: null,
    lastRunAt: null,
    latestRun: null,
    attention: null,
    revision,
    createdAt: 1,
    updatedAt
  }
}

describe('automation shared task cache ordering', () => {
  beforeEach(resetAutomationCacheForTests)

  it('does not allow an old response to replace a newer revision', () => {
    cacheAutomationTask(task(3))
    expect(cacheAutomationTask(task(2))).toBe(false)
    expect(getCachedAutomationTask('automation-1')?.revision).toBe(3)
  })

  it('accepts a fresher run projection without requiring config revision growth', () => {
    cacheAutomationTask(task(3, 10))
    expect(cacheAutomationTask(task(3, 11))).toBe(true)
    expect(getCachedAutomationTask('automation-1')?.updatedAt).toBe(11)
  })

  it('does not replace an equal-revision projection with an equally old late response', () => {
    const current = { ...task(3, 10), title: 'current projection' }
    const late = { ...task(3, 10), title: 'late projection' }
    cacheAutomationTask(current)

    expect(cacheAutomationTask(late)).toBe(false)
    expect(getCachedAutomationTask('automation-1')?.title).toBe('current projection')
  })

  it('keeps an acknowledged deletion from being resurrected by an in-flight response', () => {
    cacheAutomationTask(task(3))
    removeCachedAutomationTask('automation-1', 4)
    expect(cacheAutomationTask(task(4))).toBe(false)
    expect(getCachedAutomationTask('automation-1')).toBeNull()
  })
})
