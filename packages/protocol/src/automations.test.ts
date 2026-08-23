import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD,
  AUTOMATION_ATTENTION_SUMMARY_METHOD,
  AUTOMATION_CREATE_METHOD,
  AUTOMATION_DELETE_METHOD,
  AUTOMATION_EVENT_NOTIFICATION_METHOD,
  AUTOMATION_GET_METHOD,
  AUTOMATION_LIST_METHOD,
  AUTOMATION_NOTIFICATIONS_ACKNOWLEDGE_METHOD,
  AUTOMATION_NOTIFICATIONS_CLAIM_METHOD,
  AUTOMATION_NOTIFICATIONS_RELEASE_METHOD,
  AUTOMATION_NOTIFICATIONS_VALIDATE_METHOD,
  AUTOMATION_PERMISSION_MODE_VERSION,
  AUTOMATION_RESYNC_NOTIFICATION_METHOD,
  AUTOMATION_RUN_NOW_METHOD,
  AUTOMATION_RUNS_LIST_METHOD,
  AUTOMATION_SET_ENABLED_METHOD,
  AUTOMATION_UPDATE_METHOD,
  parseAutomationCreateInput,
  parseAutomationEvent,
  parseAutomationListInput,
  parseAutomationNotificationsClaimOutput,
  parseAutomationNotificationValidateOutput,
  parseAutomationResync,
  parseAutomationScheduleInput,
  parseAutomationTask,
  type AutomationCreateInput,
  type AutomationEvent,
  type AutomationResync,
  type AutomationTask
} from './automations'

interface AutomationContractFixture {
  permissionModeVersion: number
  permissionModes: string[]
  nullableListInput: unknown
  nullableCustomSchedule: unknown
  notificationClaimOutput: unknown
  notificationValidateOutput: unknown
  methods: Record<string, string>
  notifications: Record<string, string>
  createInput: AutomationCreateInput
  task: AutomationTask
  event: AutomationEvent
  resync: AutomationResync
}

const fixture = JSON.parse(
  readFileSync(
    fileURLToPath(new URL('../fixtures/automation-contract-v1.json', import.meta.url)),
    'utf8'
  )
) as AutomationContractFixture

describe('Automation cross-language protocol', () => {
  it('keeps methods, notifications, and permission mode version stable', () => {
    expect(fixture.methods).toEqual({
      list: AUTOMATION_LIST_METHOD,
      get: AUTOMATION_GET_METHOD,
      create: AUTOMATION_CREATE_METHOD,
      update: AUTOMATION_UPDATE_METHOD,
      setEnabled: AUTOMATION_SET_ENABLED_METHOD,
      runNow: AUTOMATION_RUN_NOW_METHOD,
      delete: AUTOMATION_DELETE_METHOD,
      listRuns: AUTOMATION_RUNS_LIST_METHOD,
      attentionSummary: AUTOMATION_ATTENTION_SUMMARY_METHOD,
      acknowledgeAttention: AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD,
      claimNotifications: AUTOMATION_NOTIFICATIONS_CLAIM_METHOD,
      validateNotification: AUTOMATION_NOTIFICATIONS_VALIDATE_METHOD,
      acknowledgeNotification: AUTOMATION_NOTIFICATIONS_ACKNOWLEDGE_METHOD,
      releaseNotification: AUTOMATION_NOTIFICATIONS_RELEASE_METHOD
    })
    expect(fixture.notifications).toEqual({
      event: AUTOMATION_EVENT_NOTIFICATION_METHOD,
      resync: AUTOMATION_RESYNC_NOTIFICATION_METHOD
    })
    expect(fixture.permissionModeVersion).toBe(AUTOMATION_PERMISSION_MODE_VERSION)
    expect(fixture.permissionModes).toEqual(['default', 'full', 'custom'])
  })

  it('strictly parses inputs, task projections, and notification envelopes', () => {
    expect(parseAutomationCreateInput(fixture.createInput)).toEqual(fixture.createInput)
    expect(parseAutomationTask(fixture.task)).toEqual(fixture.task)
    expect(parseAutomationEvent(fixture.event)).toEqual(fixture.event)
    expect(parseAutomationResync(fixture.resync)).toEqual(fixture.resync)
    expect(parseAutomationNotificationsClaimOutput(fixture.notificationClaimOutput)).toEqual(
      fixture.notificationClaimOutput
    )
    expect(parseAutomationNotificationValidateOutput(fixture.notificationValidateOutput)).toEqual(
      fixture.notificationValidateOutput
    )
  })

  it('normalizes nullable optional inputs and enforces custom frequency fields', () => {
    expect(parseAutomationListInput(fixture.nullableListInput)).toEqual({
      schemaVersion: 1,
      limit: 20
    })
    expect(parseAutomationScheduleInput(fixture.nullableCustomSchedule)).toEqual({
      kind: 'custom',
      frequency: 'hourly',
      interval: 2,
      minuteOfHour: 30,
      anchorAt: 1787414400000,
      timezone: 'Asia/Shanghai'
    })
    expect(() =>
      parseAutomationScheduleInput({
        ...fixture.nullableCustomSchedule,
        timeMinutes: 540
      })
    ).toThrow(/timeMinutes/)
  })

  it('rejects unknown fields at nested discriminated-union boundaries', () => {
    expect(() =>
      parseAutomationCreateInput({
        ...fixture.createInput,
        schedule: { ...fixture.createInput.schedule, cron: '* * * * *' }
      })
    ).toThrow(/unexpected field cron/)
    expect(() =>
      parseAutomationTask({
        ...fixture.task,
        destination: { ...fixture.task.destination, providerSecret: 'must-not-cross' }
      })
    ).toThrow(/unexpected field providerSecret/)
  })

  it('uses UTF-8 byte limits while accepting multiline prompts', () => {
    expect(
      parseAutomationCreateInput({
        ...fixture.createInput,
        prompt: 'First line\nSecond line\twith detail'
      }).prompt
    ).toBe('First line\nSecond line\twith detail')
    expect(() =>
      parseAutomationCreateInput({
        ...fixture.createInput,
        title: '中'.repeat(171)
      })
    ).toThrow(/512 UTF-8 bytes/)
    expect(() =>
      parseAutomationCreateInput({
        ...fixture.createInput,
        prompt: 'unsafe\u0000prompt'
      })
    ).toThrow(/invalid controls/)
  })

  it('rejects cross-resource task projections and empty nullable identifiers', () => {
    expect(() =>
      parseAutomationTask({
        ...fixture.task,
        timezone: 'UTC'
      })
    ).toThrow(/must match schedule.timezone/)
    expect(() =>
      parseAutomationTask({
        ...fixture.task,
        status: 'paused',
        nextRunAt: fixture.task.nextRunAt
      })
    ).toThrow(/must be null/)
    expect(() =>
      parseAutomationEvent({
        ...fixture.event,
        runId: ''
      })
    ).toThrow(/1 to 256 UTF-8 bytes/)
    const claim = fixture.notificationClaimOutput as {
      notifications: Array<Record<string, unknown>>
    }
    expect(() =>
      parseAutomationNotificationsClaimOutput({
        ...claim,
        notifications: [{ ...claim.notifications[0], runId: null }]
      })
    ).toThrow(/run notifications must identify one/)
    const validation = fixture.notificationValidateOutput as {
      notification: Record<string, unknown>
    }
    expect(() =>
      parseAutomationNotificationValidateOutput({
        ...validation,
        notification: {
          ...validation.notification,
          notificationId: 'another-notification'
        }
      })
    ).toThrow(/must match notificationId/)
    expect(() =>
      parseAutomationNotificationValidateOutput({
        ...validation,
        internalClaimState: 'must-not-cross'
      })
    ).toThrow(/unexpected field internalClaimState/)
    expect(
      parseAutomationNotificationValidateOutput({
        schemaVersion: 1,
        notificationId: 'notification-stale',
        notification: null
      })
    ).toEqual({
      schemaVersion: 1,
      notificationId: 'notification-stale',
      notification: null
    })
  })
})
