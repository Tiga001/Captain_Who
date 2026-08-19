import { describe, expect, it } from 'vitest'
import { getTranslation } from '../../../config/frontendTranslations'
import { getBuiltinCapabilityDisplayName } from '../builtinCapabilityPresentation'

describe('built-in capability presentation', () => {
  it('localizes the Host-owned browser capability identity in both supported languages', () => {
    expect(
      getBuiltinCapabilityDisplayName('browser_automation', (key) => getTranslation('zh-CN', key))
    ).toBe('浏览器自动化')
    expect(
      getBuiltinCapabilityDisplayName('browser_automation', (key) => getTranslation('en-US', key))
    ).toBe('Browser automation')
  })
})
