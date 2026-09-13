import { defineSettingsNodes } from '../settingsDefinition'

export const profileSettingsNodes = defineSettingsNodes([
  {
    id: 'profile.account',
    title: 'auth.cloudProfile',
    children: [
      {
        id: 'profile.avatar',
        title: 'profile.avatar',
        terms: ['auth.editProfile']
      },
      {
        id: 'profile.displayName',
        title: 'profile.displayName',
        description: 'auth.cloudProfile'
      },
      { id: 'profile.email', title: 'auth.email', description: 'auth.cloudProfile' }
    ]
  },
  { id: 'profile.license', title: 'license.title' },
  { id: 'profile.localTokenUsage', title: 'localUsage.title' }
])
