// Renderer startup tests: ensure the pre-React shell uses the shared translation catalogs.

import { describe, expect, it } from 'vitest'
import { getTranslation } from '../../config/languageRegistry'
import { getBootstrapStartupCopy } from '../../features/startup/bootstrapStartupLocalization'

describe('getBootstrapStartupCopy', () => {
  it('uses the saved Chinese catalog', () => {
    expect(getBootstrapStartupCopy(JSON.stringify({ language: 'zh-CN' }))).toEqual({
      ambient: getTranslation('zh-CN', 'startup.ambient.deepThinking'),
      direction: 'ltr',
      failedDescription: getTranslation('zh-CN', 'startup.failedDescription'),
      failedTitle: getTranslation('zh-CN', 'startup.failedTitle'),
      language: 'zh-CN',
      loading: getTranslation('zh-CN', 'startup.loading')
    })
  })

  it('uses the saved English catalog', () => {
    expect(getBootstrapStartupCopy(JSON.stringify({ language: 'en-US' }))).toEqual({
      ambient: getTranslation('en-US', 'startup.ambient.deepThinking'),
      direction: 'ltr',
      failedDescription: getTranslation('en-US', 'startup.failedDescription'),
      failedTitle: getTranslation('en-US', 'startup.failedTitle'),
      language: 'en-US',
      loading: getTranslation('en-US', 'startup.loading')
    })
  })

  it('uses a newly registered catalog before React mounts', () => {
    expect(getBootstrapStartupCopy(JSON.stringify({ language: 'ja-JP' }))).toEqual({
      ambient: getTranslation('ja-JP', 'startup.ambient.deepThinking'),
      direction: 'ltr',
      failedDescription: getTranslation('ja-JP', 'startup.failedDescription'),
      failedTitle: getTranslation('ja-JP', 'startup.failedTitle'),
      language: 'ja-JP',
      loading: getTranslation('ja-JP', 'startup.loading')
    })
  })

  it.each([null, '{invalid json', JSON.stringify({ language: 'unsupported' })])(
    'falls back safely for invalid stored configuration: %s',
    (rawStoredConfig) => {
      expect(getBootstrapStartupCopy(rawStoredConfig).language).toBe('zh-CN')
    }
  )
})
