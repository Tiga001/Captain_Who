import { defineSettingsNodes } from '../settingsDefinition'

export const USAGE_RANGE_OPTIONS = [
  { labelKey: 'usageBilling.rangeLast7Days', value: 'last7Days' },
  { labelKey: 'usageBilling.rangeLast30Days', value: 'last30Days' },
  { labelKey: 'usageBilling.rangeLastYear', value: 'lastYear' }
] as const

export const usageBillingSettingsNodes = defineSettingsNodes([
  {
    id: 'usageBilling.summary',
    title: 'usageBilling.summary',
    children: [
      { id: 'usageBilling.clear', title: 'usageBilling.clear' },
      {
        id: 'usageBilling.usageTime',
        title: 'usageBilling.usageTime',
        terms: USAGE_RANGE_OPTIONS.map((option) => option.labelKey)
      },
      { id: 'usageBilling.model', title: 'usageBilling.model' },
      { id: 'usageBilling.models', title: 'usageBilling.models' }
    ]
  },
  {
    id: 'usageBilling.displaySettings',
    title: 'usageBilling.displaySettings',
    children: [
      {
        id: 'usageBilling.tokenDetails',
        title: 'usageBilling.tokenDetails',
        description: 'usageBilling.tokenDetailsDescription',
        preferenceKey: 'showTokenUsageDetails',
        headingId: 'usage-token-details-heading'
      },
      {
        id: 'usageBilling.showCacheHitRate',
        title: 'usageBilling.showCacheHitRate',
        description: 'usageBilling.showCacheHitRateDescription',
        preferenceKey: 'showCacheHitRate',
        headingId: 'usage-cache-hit-rate-heading'
      }
    ]
  }
])
