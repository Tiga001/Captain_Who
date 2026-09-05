import { defineSettingsNodes } from '../settings/settingsDefinition'

export const browserClearTimeRangeSettings = defineSettingsNodes([
  {
    id: 'browser-data-range-last-hour',
    title: 'mcp.browserData.range.lastHour',
    range: 'lastHour',
    view: 'clearData'
  },
  {
    id: 'browser-data-range-last-day',
    title: 'mcp.browserData.range.last24Hours',
    range: 'last24Hours',
    view: 'clearData'
  },
  {
    id: 'browser-data-range-last-week',
    title: 'mcp.browserData.range.last7Days',
    range: 'last7Days',
    view: 'clearData'
  },
  {
    id: 'browser-data-range-last-month',
    title: 'mcp.browserData.range.last4Weeks',
    range: 'last4Weeks',
    view: 'clearData'
  },
  {
    id: 'browser-data-range-all-time',
    title: 'mcp.browserData.range.allTime',
    range: 'allTime',
    view: 'clearData'
  }
])

export const browserClearCategorySettings = defineSettingsNodes([
  {
    id: 'browser-data-history',
    title: 'mcp.browserData.history',
    category: 'history',
    view: 'clearData'
  },
  {
    id: 'browser-data-cookies',
    title: 'mcp.browserData.cookies',
    category: 'cookiesAndSiteData',
    view: 'clearData'
  },
  {
    id: 'browser-data-cache',
    title: 'mcp.browserData.cache',
    category: 'cache',
    view: 'clearData'
  },
  {
    id: 'browser-data-download-history',
    title: 'mcp.browserData.downloadHistory',
    category: 'downloadHistory',
    view: 'clearData'
  }
])

export const browserHistorySettings = defineSettingsNodes([
  { id: 'browser-history-search', title: 'mcp.browserData.searchHistory', view: 'history' },
  { id: 'browser-history-clear', title: 'browser.clearBrowsingData', view: 'history' }
])

export const browserDownloadHistorySettings = defineSettingsNodes([
  {
    id: 'browser-download-history-search',
    title: 'mcp.browserDownloads.search',
    view: 'downloadHistory'
  },
  {
    id: 'browser-download-history-clear',
    title: 'mcp.browserDownloads.clearHistory',
    view: 'downloadHistory'
  }
])

export const browserAutomationSettings = defineSettingsNodes([
  {
    id: 'browser-automation',
    title: 'mcp.builtin.browserAutomation.name',
    description: 'mcp.builtin.browserAutomation.description',
    view: 'settings'
  }
])

export const browserGeneralSettings = defineSettingsNodes([
  {
    id: 'browser-link-target',
    title: 'mcp.browserData.linkTarget',
    description: 'mcp.browserData.linkTargetDescription',
    terms: ['mcp.browserData.systemBrowser', 'mcp.browserData.builtinBrowser']
  },
  {
    id: 'browser-data',
    searchable: true,
    title: 'mcp.browserData.data',
    description: 'mcp.browserData.dataDescription',
    terms: ['browser.clearBrowsingData'],
    children: [...browserClearTimeRangeSettings, ...browserClearCategorySettings]
  },
  {
    id: 'browser-history',
    searchable: true,
    title: 'browser.history',
    description: 'mcp.browserData.historyDescription',
    children: browserHistorySettings
  }
])

export const browserDownloadSettings = defineSettingsNodes([
  {
    id: 'browser-download-location',
    title: 'mcp.browserDownloads.location',
    terms: [
      'mcp.browserDownloads.systemLocation',
      'mcp.browserDownloads.useSystemLocation',
      'mcp.browserDownloads.changeLocation'
    ]
  },
  { id: 'browser-download-ask-where', title: 'mcp.browserDownloads.askWhereToSave' },
  {
    id: 'browser-download-history',
    searchable: true,
    title: 'mcp.browserDownloads.history',
    children: browserDownloadHistorySettings
  }
])

export const browserSettings = defineSettingsNodes([
  ...browserAutomationSettings,
  {
    id: 'browser-general',
    title: 'mcp.browserData.general',
    view: 'settings',
    children: browserGeneralSettings
  },
  {
    id: 'browser-downloads',
    title: 'mcp.browserDownloads.section',
    view: 'settings',
    children: browserDownloadSettings
  }
])
