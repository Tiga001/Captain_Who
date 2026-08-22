// Renderer Skill presentation regressions for localized bundled identities and external fallbacks.
import type { SkillSourceDescriptor } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import { getTranslation } from '../../../config/languageRegistry'
import { getSkillPresentation, sortSkillsForDisplay } from '../skillPresentation'

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
  },
  {
    id: 'bundled:application:pdf',
    sourceId: 'application:pdf',
    zhName: 'PDF',
    enName: 'PDF'
  },
  {
    id: 'bundled:application:skill-installer',
    sourceId: 'application:skill-installer',
    zhName: 'Skill 安装器',
    enName: 'Skill Installer'
  },
  {
    id: 'bundled:application:skill-creator',
    sourceId: 'application:skill-creator',
    zhName: 'Skill 创建器',
    enName: 'Skill Creator'
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

  it('localizes only the trusted bundled Skill Installer identity', () => {
    const bundled = {
      description: 'Backend description',
      id: 'bundled:application:skill-installer',
      name: 'skill-installer',
      source: { id: 'application:skill-installer', kind: 'bundled' } as SkillSourceDescriptor
    }
    const installed = {
      description: 'Third-party installer description',
      id: 'installed:user:skill-installer',
      name: 'skill-installer',
      source: { id: 'installed:user', kind: 'installed' } as SkillSourceDescriptor
    }

    expect(getSkillPresentation(bundled, (key) => getTranslation('zh-CN', key))).toEqual({
      description: '检查并安装来自 GitHub 链接或已授权本地路径且无需修改的第三方 Skill。',
      name: 'Skill 安装器'
    })
    expect(getSkillPresentation(bundled, (key) => getTranslation('en-US', key))).toEqual({
      description:
        'Inspect and install unchanged third-party Skills from GitHub links or authorized local paths.',
      name: 'Skill Installer'
    })
    expect(getSkillPresentation(installed, (key) => getTranslation('zh-CN', key))).toEqual({
      description: installed.description,
      name: installed.name
    })
  })
})

describe('bundled Skill display order', () => {
  it('lists installer, creator, image generation, Word, PPT, Excel, then PDF before other skills', () => {
    const ordered = sortSkillsForDisplay([
      {
        id: 'bundled:application:documents',
        name: 'Documents',
        source: { id: 'application:documents', kind: 'bundled' }
      },
      {
        id: 'bundled:application:image-generation',
        name: 'Image Generation',
        source: { id: 'application:image-generation', kind: 'bundled' }
      },
      {
        id: 'bundled:application:pdf',
        name: 'PDF',
        source: { id: 'application:pdf', kind: 'bundled' }
      },
      {
        id: 'bundled:application:presentations',
        name: 'Presentations',
        source: { id: 'application:presentations', kind: 'bundled' }
      },
      {
        id: 'bundled:application:skill-installer',
        name: 'Skill Installer',
        source: { id: 'application:skill-installer', kind: 'bundled' }
      },
      {
        id: 'bundled:application:skill-creator',
        name: 'Skill Creator',
        source: { id: 'application:skill-creator', kind: 'bundled' }
      },
      {
        id: 'installed:user:custom-skill',
        name: 'Custom skill',
        source: { id: 'installed:user', kind: 'installed' }
      },
      {
        id: 'bundled:application:spreadsheets',
        name: 'Spreadsheets',
        source: { id: 'application:spreadsheets', kind: 'bundled' }
      }
    ] as const)

    expect(ordered.map((skill) => skill.id)).toEqual([
      'bundled:application:skill-installer',
      'bundled:application:skill-creator',
      'bundled:application:image-generation',
      'bundled:application:documents',
      'bundled:application:presentations',
      'bundled:application:spreadsheets',
      'bundled:application:pdf',
      'installed:user:custom-skill'
    ])
  })
})
