// Renderer Skill presentation regressions for localized bundled identities and external fallbacks.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { getTranslation } from '../../../config/languageRegistry'
import { getSkillPresentation } from '../skillPresentation'

const bundledCases = [
  {
    id: 'bundled:application:documents',
    sourceId: 'application:documents',
    zhName: '文档',
    enName: 'Documents'
  },
  {
    id: 'bundled:application:spreadsheets',
    sourceId: 'application:spreadsheets',
    zhName: '电子表格',
    enName: 'Spreadsheets'
  },
  {
    id: 'bundled:application:presentations',
    sourceId: 'application:presentations',
    zhName: '演示文稿',
    enName: 'Presentations'
  },
  {
    id: 'bundled:application:image-generation',
    sourceId: 'application:image-generation',
    zhName: '图片生成',
    enName: 'Image Generation'
  }
] as const

describe('bundled Skill presentation', () => {
  it('localizes every known bundled Skill name and description in Chinese and English', () => {
    for (const skill of bundledCases) {
      const input = {
        description: 'Backend description',
        id: skill.id,
        name: 'Backend name',
        source: { id: skill.sourceId, kind: 'bundled' } as SkillSourceDescriptor
      }
      const chinese = getSkillPresentation(input, (key) => getTranslation('zh-CN', key))
      const english = getSkillPresentation(input, (key) => getTranslation('en-US', key))

      expect(chinese.name).toBe(skill.zhName)
      expect(chinese.description).not.toBe(input.description)
      expect(english.name).toBe(skill.enName)
      expect(english.description).not.toBe(input.description)
    }
  })

  it('preserves installed and unknown Skill package presentation', () => {
    const input = {
      description: 'Package-provided description',
      id: 'installed:user:custom-skill',
      name: 'Custom skill',
      source: { id: 'installed:user', kind: 'installed' } as SkillSourceDescriptor
    }

    expect(getSkillPresentation(input, (key) => getTranslation('zh-CN', key))).toEqual({
      description: input.description,
      name: input.name
    })
  })
})
