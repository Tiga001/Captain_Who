import { isMacOS } from '../../../lib/platform'
import { defineSettingsNodes } from '../settingsDefinition'

export const COLOR_SCHEME_OPTIONS = [
  { id: 'system', labelKey: 'appearance.theme.system' },
  { id: 'light', labelKey: 'appearance.theme.light' },
  { id: 'dark', labelKey: 'appearance.theme.dark' }
] as const

export const appearanceSettingsNodes = defineSettingsNodes([
  {
    id: 'appearance.theme',
    title: 'appearance.theme',
    searchable: true,
    terms: COLOR_SCHEME_OPTIONS.map((option) => option.labelKey),
    children: [
      {
        id: 'appearance.lightTheme',
        title: 'appearance.themeVariant.light',
        prerequisiteId: 'appearance.theme'
      },
      {
        id: 'appearance.darkTheme',
        title: 'appearance.themeVariant.dark',
        prerequisiteId: 'appearance.theme'
      }
    ]
  },
  {
    id: 'appearance.preferences',
    title: 'appearance.preferences',
    children: [
      {
        id: 'appearance.nativeFontSmoothing',
        title: 'appearance.nativeFontSmoothing',
        description: 'appearance.nativeFontSmoothingDescription',
        supported: isMacOS
      },
      {
        id: 'appearance.translucentSidebar',
        title: 'appearance.translucentSidebar',
        description: 'appearance.translucentSidebarDescription'
      },
      {
        id: 'appearance.translucentSidebarTransparency',
        title: 'appearance.translucentSidebarTransparency',
        prerequisiteId: 'appearance.translucentSidebar'
      }
    ]
  }
])
