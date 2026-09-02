import { describe, expect, it, vi } from 'vitest'
import { zhCNTranslations } from '../../../config/frontendTranslations.zhCN'
import {
  NEW_CONVERSATION_PROMPTS,
  getNewConversationPromptKeys,
  getNextRandomNewConversationPromptIndex,
  getRandomNewConversationPromptIndex
} from '../newConversationPrompts'

const EXPECTED_ZH_CN_PROMPTS = [
  ['我们应该驶向何方？', '指引方向'],
  ['今天想构建什么？', '描绘你的蓝图'],
  ['准备解决什么问题？', '从问题开始'],
  ['这次想探索什么？', '随心输入'],
  ['下一站是什么？', '告诉我你的计划']
] as const

describe('newConversationPrompts', () => {
  it('keeps the five approved title and placeholder combinations paired', () => {
    expect(NEW_CONVERSATION_PROMPTS).toHaveLength(EXPECTED_ZH_CN_PROMPTS.length)

    expect(
      NEW_CONVERSATION_PROMPTS.map(({ placeholderKey, titleKey }) => [
        zhCNTranslations[titleKey],
        zhCNTranslations[placeholderKey]
      ])
    ).toEqual(EXPECTED_ZH_CN_PROMPTS)

    for (const [index, prompt] of NEW_CONVERSATION_PROMPTS.entries()) {
      expect(getNewConversationPromptKeys(index)).toEqual(prompt)
    }
  })

  it.each([
    [0, 0],
    [0.199_999, 0],
    [0.2, 1],
    [0.399_999, 1],
    [0.4, 2],
    [0.599_999, 2],
    [0.6, 3],
    [0.799_999, 3],
    [0.8, 4],
    [0.999_999, 4]
  ])('maps random boundary %f to prompt index %i', (randomValue, expectedIndex) => {
    const random = vi.fn(() => randomValue)

    expect(getRandomNewConversationPromptIndex(random)).toBe(expectedIndex)
    expect(random).toHaveBeenCalledOnce()
  })

  it('selects a valid different prompt for every current index and random bucket', () => {
    const randomValues = [0, 0.249_999, 0.25, 0.499_999, 0.5, 0.749_999, 0.75, 0.999_999]

    for (const currentIndex of NEW_CONVERSATION_PROMPTS.keys()) {
      for (const randomValue of randomValues) {
        const random = vi.fn(() => randomValue)
        const nextIndex = getNextRandomNewConversationPromptIndex(currentIndex, random)

        expect(nextIndex).toBeGreaterThanOrEqual(0)
        expect(nextIndex).toBeLessThan(NEW_CONVERSATION_PROMPTS.length)
        expect(nextIndex).not.toBe(currentIndex)
        expect(random).toHaveBeenCalledOnce()
      }
    }
  })
})
