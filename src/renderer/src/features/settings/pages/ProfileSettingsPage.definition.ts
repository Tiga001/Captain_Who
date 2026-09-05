import { defineSettingsNodes } from '../settingsDefinition'

export const profileSettingsNodes = defineSettingsNodes([
  {
    id: 'profile.account',
    title: 'profile.account',
    children: [
      {
        id: 'profile.avatar',
        title: 'profile.avatar',
        terms: ['profile.uploadAvatar', 'profile.removeAvatar']
      },
      {
        id: 'profile.displayName',
        title: 'profile.displayName',
        description: 'profile.displayNameDescription'
      },
      { id: 'profile.handle', title: 'profile.handle', description: 'profile.handleDescription' }
    ]
  }
])
