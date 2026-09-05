import { featureFlags } from '../../../config/featureFlags'
import { appLanguageOptions } from '../../../config/languageRegistry'
import { hasNotificationHostApi } from '../../notifications/notificationClient'
import { defineSettingsNodes } from '../settingsDefinition'

export const READ_PERMISSION_OPTIONS = [
  { value: 'workspace_only', label: 'general.workspaceOnly' },
  { value: 'all', label: 'general.allLocations' }
] as const
export const WRITE_PERMISSION_OPTIONS = [
  { value: 'denied', label: 'general.writeDenied' },
  ...READ_PERMISSION_OPTIONS
] as const
export const NOTIFICATION_MODE_OPTIONS = [
  {
    value: 'never',
    label: 'notification.ordinaryModeNever',
    description: 'notification.ordinaryModeNeverDescription'
  },
  {
    value: 'all',
    label: 'notification.ordinaryModeAll',
    description: 'notification.ordinaryModeAllDescription'
  },
  {
    value: 'necessary',
    label: 'notification.ordinaryModeNecessary',
    description: 'notification.ordinaryModeNecessaryDescription'
  },
  {
    value: 'custom',
    label: 'notification.ordinaryModeCustom',
    description: 'notification.ordinaryModeCustomDescription'
  }
] as const

export const generalSettingsNodes = defineSettingsNodes([
  {
    id: 'general.preferences',
    title: 'general.sectionGeneral',
    children: [
      {
        id: 'general.language',
        title: 'general.language',
        terms: appLanguageOptions.map((option) => ({ text: option.label }))
      }
    ]
  },
  {
    id: 'general.permissions',
    title: 'general.sectionPermissions',
    children: [
      {
        id: 'general.defaultPermission',
        title: 'chat.defaultPermission',
        description: 'general.defaultPermissionDescription'
      },
      {
        id: 'general.fullPermission',
        title: 'chat.fullPermission',
        description: 'general.fullPermissionDescription'
      },
      {
        id: 'general.customPermission',
        title: 'chat.customPermission',
        description: 'general.customPermissionDescription'
      }
    ]
  },
  {
    id: 'general.customPermissions',
    title: 'general.customPermissions',
    children: [
      {
        id: 'general.readPermission',
        title: 'general.readPermission',
        terms: READ_PERMISSION_OPTIONS.map((option) => option.label)
      },
      {
        id: 'general.writePermission',
        title: 'general.writePermission',
        terms: WRITE_PERMISSION_OPTIONS.map((option) => option.label)
      },
      {
        id: 'general.autoApproveFileEdits',
        title: 'general.autoApproveFileEdits',
        description: 'general.autoApproveFileEditsDescription',
        prerequisiteId: 'general.writePermission'
      },
      {
        id: 'general.autoApproveCommands',
        title: 'general.autoApproveCommands',
        description: 'general.autoApproveCommandsDescription'
      },
      {
        id: 'general.autoApproveBuiltinExecution',
        title: 'general.autoApproveBuiltinExecution',
        description: 'general.autoApproveBuiltinExecutionDescription'
      }
    ]
  },
  {
    id: 'general.composer',
    title: 'general.sectionComposer',
    supported: () => featureFlags.contextWindowIndicator,
    children: [{ id: 'general.showContextWindowUsage', title: 'general.showContextWindowUsage' }]
  },
  {
    id: 'notifications',
    title: 'general.sectionNotifications',
    supported: hasNotificationHostApi,
    children: [
      {
        id: 'notifications.mode',
        title: 'notification.ordinaryModeLabel',
        terms: NOTIFICATION_MODE_OPTIONS.flatMap((option) => [option.label, option.description])
      },
      {
        id: 'notifications.custom',
        title: 'notification.ordinaryCustomAria',
        searchable: false,
        children: [
          {
            id: 'notifications.humanCompletedEnabled',
            title: 'notification.settingCompleted',
            description: 'notification.settingCompletedDescription',
            view: 'notificationCustom'
          },
          {
            id: 'notifications.humanFailedEnabled',
            title: 'notification.settingFailed',
            description: 'notification.settingFailedDescription',
            view: 'notificationCustom'
          },
          {
            id: 'notifications.humanApprovalEnabled',
            title: 'notification.settingApproval',
            description: 'notification.settingApprovalDescription',
            view: 'notificationCustom'
          },
          {
            id: 'notifications.humanCancelledEnabled',
            title: 'notification.settingCancelled',
            description: 'notification.settingCancelledDescription',
            view: 'notificationCustom'
          }
        ]
      },
      {
        id: 'notifications.soundEnabled',
        title: 'notification.settingSound',
        description: 'notification.settingSoundDescription'
      },
      {
        id: 'notifications.showTaskContent',
        title: 'notification.settingPreview',
        description: 'notification.settingPreviewDescription'
      }
    ]
  }
])
