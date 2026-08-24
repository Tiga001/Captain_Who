import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  NOTIFICATION_BATCHES_CLAIM_METHOD,
  NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD,
  NOTIFICATION_BATCH_RELEASE_METHOD,
  NOTIFICATION_BATCH_SUPPRESS_METHOD,
  NOTIFICATION_BATCH_VALIDATE_METHOD,
  NOTIFICATION_EVENT_NOTIFICATION_METHOD,
  NOTIFICATION_LIST_METHOD,
  NOTIFICATION_MARK_SEEN_METHOD,
  NOTIFICATION_RESYNC_NOTIFICATION_METHOD,
  NOTIFICATION_SETTINGS_GET_METHOD,
  NOTIFICATION_SETTINGS_UPDATE_METHOD,
  NOTIFICATION_SUMMARY_METHOD,
  parseNotificationBatch,
  parseNotificationEvent,
  parseNotificationMarkSeenInput,
  parseNotificationOpenRequest,
  parseNotificationResync,
  parseNotificationSettings
} from './notifications'

const fixture = JSON.parse(
  readFileSync(
    fileURLToPath(new URL('../fixtures/notification-contract-v1.json', import.meta.url)),
    'utf8'
  )
) as Record<string, unknown>

describe('Application notification protocol', () => {
  it('keeps methods stable across Host and Core', () => {
    expect(fixture.methods).toEqual({
      claim: NOTIFICATION_BATCHES_CLAIM_METHOD,
      validate: NOTIFICATION_BATCH_VALIDATE_METHOD,
      acknowledge: NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD,
      release: NOTIFICATION_BATCH_RELEASE_METHOD,
      suppress: NOTIFICATION_BATCH_SUPPRESS_METHOD,
      list: NOTIFICATION_LIST_METHOD,
      summary: NOTIFICATION_SUMMARY_METHOD,
      markSeen: NOTIFICATION_MARK_SEEN_METHOD,
      getSettings: NOTIFICATION_SETTINGS_GET_METHOD,
      updateSettings: NOTIFICATION_SETTINGS_UPDATE_METHOD
    })
    expect(fixture.notifications).toEqual({
      event: NOTIFICATION_EVENT_NOTIFICATION_METHOD,
      resync: NOTIFICATION_RESYNC_NOTIFICATION_METHOD
    })
  })

  it('strictly parses batches, settings, events, and resync', () => {
    expect(parseNotificationBatch(fixture.batch)).toEqual(fixture.batch)
    expect(parseNotificationSettings(fixture.settings)).toEqual(fixture.settings)
    expect(parseNotificationEvent(fixture.event)).toEqual(fixture.event)
    expect(parseNotificationResync(fixture.resync)).toEqual(fixture.resync)
    expect(parseNotificationOpenRequest(fixture.openRequest)).toEqual(fixture.openRequest)
  })

  it('keeps seen and resolution semantics separate', () => {
    expect(
      parseNotificationMarkSeenInput({
        schemaVersion: 1,
        target: { kind: 'batch', batchId: 'notification-batch:1' }
      })
    ).toEqual({
      schemaVersion: 1,
      target: { kind: 'batch', batchId: 'notification-batch:1' }
    })
  })

  it('rejects unknown delivery fields', () => {
    expect(() =>
      parseNotificationBatch({ ...(fixture.batch as object), rawError: 'secret' })
    ).toThrow(/unexpected field rawError/)
  })

  it('requires bounded unique event snapshots for clicks and seen updates', () => {
    expect(() =>
      parseNotificationOpenRequest({
        ...(fixture.openRequest as object),
        eventIds: ['notification-event:1', 'notification-event:1']
      })
    ).toThrow(/unique/)
    expect(() =>
      parseNotificationMarkSeenInput({
        schemaVersion: 1,
        target: { kind: 'events', eventIds: Array.from({ length: 101 }, (_, i) => `event-${i}`) }
      })
    ).toThrow(/1\.\.100/)
  })

  it('supports focus-only clicks and rejects the removed notification-center destination', () => {
    expect(
      parseNotificationOpenRequest({
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1'],
        destination: { kind: 'application' }
      })
    ).toEqual({
      schemaVersion: 1,
      batchId: 'batch-1',
      eventIds: ['event-1'],
      destination: { kind: 'application' }
    })
    expect(() =>
      parseNotificationOpenRequest({
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['event-1'],
        destination: { kind: 'notification_center' }
      })
    ).toThrow(/destination\.kind/)
  })
})
