import { describe, expect, it } from 'vitest'
import type { TranslationKey } from '../../../config/frontendTranslations'
import type { Translate } from '../../../config/translationFormat'
import { healthMessage, localizedScheduleSummary, runErrorMessage } from '../automationPresentation'
import { makeAutomationRun, makeAutomationTask } from './automationUiFixtures'

const translations: Partial<Record<TranslationKey, string>> = {
  'automation.healthPermissionDisabled': '权限已经关闭',
  'automation.summaryDaily': '每天 {time}',
  'automation.runErrorPermission': '运行权限已经关闭'
}
const t: Translate = (key) => translations[key] ?? key

describe('automation presentation', () => {
  it('formats the structured schedule locally instead of using the backend summary language', () => {
    const task = makeAutomationTask({ scheduleSummary: 'Daily at 09:00' })
    expect(localizedScheduleSummary(task, t, 'zh-CN')).toBe('每天 09:00')
  })

  it('maps blocked and run failure codes to frontend translations', () => {
    expect(
      healthMessage(t, {
        state: 'blocked',
        code: 'permission_disabled',
        message: 'Permission disabled'
      })
    ).toBe('权限已经关闭')
    expect(
      runErrorMessage(
        t,
        makeAutomationRun({
          status: 'failed',
          errorCode: 'permission_disabled',
          errorMessage: 'Permission disabled'
        })
      )
    ).toBe('运行权限已经关闭')
  })
})
