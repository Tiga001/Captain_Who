import { describe, expect, it } from 'vitest'
import {
  deriveOrdinaryNotificationMode,
  ordinaryNotificationPatchForMode,
  type OrdinaryNotificationModeSettings
} from '../ordinaryNotificationMode'

function settings(
  overrides: Partial<OrdinaryNotificationModeSettings> = {}
): OrdinaryNotificationModeSettings {
  return {
    enabled: true,
    humanCompletedEnabled: true,
    humanFailedEnabled: true,
    humanApprovalEnabled: true,
    humanCancelledEnabled: true,
    ...overrides
  }
}

describe('ordinary notification modes', () => {
  it('treats the legacy global-off state as never regardless of human task flags', () => {
    expect(deriveOrdinaryNotificationMode(settings({ enabled: false }))).toBe('never')
  })

  it('recognizes each canonical preset', () => {
    expect(
      deriveOrdinaryNotificationMode(
        settings({
          humanCompletedEnabled: false,
          humanFailedEnabled: false,
          humanApprovalEnabled: false,
          humanCancelledEnabled: false
        })
      )
    ).toBe('never')

    expect(deriveOrdinaryNotificationMode(settings())).toBe('all')

    expect(deriveOrdinaryNotificationMode(settings({ humanCompletedEnabled: false }))).toBe(
      'necessary'
    )
  })

  it('keeps every non-preset flag combination in custom mode', () => {
    for (let mask = 0; mask < 16; mask += 1) {
      const completed = Boolean(mask & 0b1000)
      const failed = Boolean(mask & 0b0100)
      const approval = Boolean(mask & 0b0010)
      const cancelled = Boolean(mask & 0b0001)
      const isNever = mask === 0b0000
      const isAll = mask === 0b1111
      const isNecessary = mask === 0b0111

      if (isNever || isAll || isNecessary) continue

      expect(
        deriveOrdinaryNotificationMode(
          settings({
            humanCompletedEnabled: completed,
            humanFailedEnabled: failed,
            humanApprovalEnabled: approval,
            humanCancelledEnabled: cancelled
          })
        )
      ).toBe('custom')
    }
  })

  it('maps all preset modes to complete server patches', () => {
    expect(ordinaryNotificationPatchForMode('never')).toEqual({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false
    })
    expect(ordinaryNotificationPatchForMode('all')).toEqual({
      enabled: true,
      humanCompletedEnabled: true,
      humanFailedEnabled: true,
      humanApprovalEnabled: true,
      humanCancelledEnabled: true
    })
    expect(ordinaryNotificationPatchForMode('necessary')).toEqual({
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: true,
      humanApprovalEnabled: true,
      humanCancelledEnabled: true
    })
  })

  it('keeps the notification pipeline enabled when ordinary task notifications are disabled', () => {
    expect(ordinaryNotificationPatchForMode('never').enabled).toBe(true)
  })
})
