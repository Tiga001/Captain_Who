import { describe, expect, it } from 'vitest'
import { enUSTranslations } from '../../../config/frontendTranslations.enUS'
import { zhCNTranslations } from '../../../config/frontendTranslations.zhCN'

describe('automation translations', () => {
  it('keeps product-facing automation copy brand-neutral', () => {
    for (const translations of [zhCNTranslations, enUSTranslations]) {
      const automationCopy = Object.entries(translations)
        .filter(([key]) => key.startsWith('automation.'))
        .map(([, value]) => value)
        .join('\n')

      expect(automationCopy).not.toMatch(/my\s*copilot/i)
    }
  })

  it('uses neutral descriptions for the page, empty state, and task description', () => {
    expect(zhCNTranslations['automation.pageDescription']).toBe(
      '安排任务、持续跟进工作并查看运行结果'
    )
    expect(zhCNTranslations['automation.emptyDescription']).toBe(
      '可以定时创建新聊天，也可以持续跟进一个现有聊天。'
    )
    expect(zhCNTranslations['automation.promptPlaceholder']).toBe(
      '说明这项任务每次运行时需要完成的内容'
    )
    expect(zhCNTranslations['automation.prompt']).toBe('任务描述')
    expect(zhCNTranslations['automation.validationPromptRequired']).toBe('请填写任务描述。')
    expect(zhCNTranslations['automation.permissionDefault']).toBe('默认')
    expect(zhCNTranslations['automation.permissionFull']).toBe('完全权限')
    expect(zhCNTranslations['automation.permissionCustom']).toBe('自定义')
    expect(enUSTranslations['automation.pageDescription']).toBe(
      'Schedule tasks, follow work over time, and review results'
    )
    expect(enUSTranslations['automation.emptyDescription']).toBe(
      'Create new chats on a schedule or keep following an existing chat over time.'
    )
    expect(enUSTranslations['automation.promptPlaceholder']).toBe(
      'Describe what this task should do each time it runs'
    )
    expect(enUSTranslations['automation.prompt']).toBe('Task description')
    expect(enUSTranslations['automation.validationPromptRequired']).toBe(
      'Enter a task description.'
    )
    expect(enUSTranslations['automation.permissionDefault']).toBe('Default')
    expect(enUSTranslations['automation.permissionFull']).toBe('Full access')
    expect(enUSTranslations['automation.permissionCustom']).toBe('Custom')
  })
})
