import type { NotificationSettings } from '@mycopilot/protocol'
import type { NotificationSettingsDraft } from './notificationClient'

export type OrdinaryNotificationMode = 'never' | 'all' | 'necessary' | 'custom'

export type OrdinaryNotificationPresetMode = Exclude<OrdinaryNotificationMode, 'custom'>

export type OrdinaryNotificationModeSettings = Pick<
  NotificationSettings,
  | 'enabled'
  | 'humanCompletedEnabled'
  | 'humanFailedEnabled'
  | 'humanApprovalEnabled'
  | 'humanCancelledEnabled'
>

export type OrdinaryNotificationModePatch = Pick<
  NotificationSettingsDraft,
  | 'enabled'
  | 'humanCompletedEnabled'
  | 'humanFailedEnabled'
  | 'humanApprovalEnabled'
  | 'humanCancelledEnabled'
>

export function deriveOrdinaryNotificationMode(
  settings: OrdinaryNotificationModeSettings
): OrdinaryNotificationMode {
  if (!settings.enabled) return 'never'

  const {
    humanCompletedEnabled: completed,
    humanFailedEnabled: failed,
    humanApprovalEnabled: approval,
    humanCancelledEnabled: cancelled
  } = settings

  if (!completed && !failed && !approval && !cancelled) return 'never'
  if (completed && failed && approval && cancelled) return 'all'
  if (!completed && failed && approval && cancelled) return 'necessary'
  return 'custom'
}

export function ordinaryNotificationPatchForMode(
  mode: OrdinaryNotificationPresetMode
): OrdinaryNotificationModePatch {
  if (mode === 'never') {
    return {
      enabled: true,
      humanCompletedEnabled: false,
      humanFailedEnabled: false,
      humanApprovalEnabled: false,
      humanCancelledEnabled: false
    }
  }

  if (mode === 'all') {
    return {
      enabled: true,
      humanCompletedEnabled: true,
      humanFailedEnabled: true,
      humanApprovalEnabled: true,
      humanCancelledEnabled: true
    }
  }

  return {
    enabled: true,
    humanCompletedEnabled: false,
    humanFailedEnabled: true,
    humanApprovalEnabled: true,
    humanCancelledEnabled: true
  }
}
