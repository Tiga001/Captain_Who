import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import { getTranslation } from '../../../config/frontendTranslations'
import {
  defineSettingsNodes,
  renderSettingsNodes,
  settingLabel,
  type SettingsNode
} from '../settingsDefinition'
import { buildSettingsSearchIndex, searchSettings } from '../settingsSearch'
import { SETTINGS_GROUPS } from '../settingsRegistry'

vi.mock('../../notifications/notificationClient', () => ({
  hasNotificationHostApi: () => false
}))

const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)

describe('settings definitions as the shared rendering and search source', () => {
  it('adds, renames, moves and removes UI fields and search results from one definition', () => {
    const original = defineSettingsNodes([{ id: 'sample', title: { text: '旧名称' } }])
    const changed = defineSettingsNodes([{ id: 'new-field', title: { text: '新名称' } }])
    const build = (nodes: readonly SettingsNode[]) => ({
      markup: renderToStaticMarkup(
        <>
          {renderSettingsNodes(nodes, (node) => (
            <label className="existing-field">{settingLabel(node, t)}</label>
          ))}
        </>
      ),
      index: buildSettingsSearchIndex(
        [{ id: 'general', labelKey: 'settings.page.general', nodes }],
        t
      )
    })
    expect(build(original).markup).toContain('旧名称')
    const next = build(changed)
    expect(next.markup).not.toContain('旧名称')
    expect(next.markup).toContain('data-setting-id="new-field"')
    expect(next.markup).toContain('新名称')
    expect(searchSettings(next.index, '旧名称')).toEqual([])
    expect(searchSettings(next.index, '新名称').map((item) => item.id)).toEqual(['new-field'])
    const moved = buildSettingsSearchIndex(
      [{ id: 'appearance', labelKey: 'settings.page.appearance', nodes: changed }],
      t
    )
    expect(searchSettings(moved, '新名称')[0].path).toEqual(['外观'])
    expect(build([])).toEqual({ markup: '', index: [] })
  })

  it('indexes all 13 page definitions without mounting their components', () => {
    const pages = SETTINGS_GROUPS.flatMap((group) => group.items)
    expect(pages).toHaveLength(13)
    const index = buildSettingsSearchIndex(pages, t)
    expect(new Set(index.map((entry) => entry.page)).size).toBe(13)
    expect(
      searchSettings(index, '审批').some((entry) => entry.id === 'general.autoApproveCommands')
    ).toBe(true)
    expect(
      searchSettings(index, 'API').some((entry) => entry.id === 'configuration.apiToken')
    ).toBe(true)
    expect(searchSettings(index, '提问').some((entry) => entry.page === 'personalization')).toBe(
      true
    )
  })

  it('places workflows beside subagents and targets its own library and editor', () => {
    const pages = SETTINGS_GROUPS.flatMap((group) => group.items)
    const agentIndex = pages.findIndex((page) => page.id === 'agentTemplates')
    expect(pages[agentIndex + 1].id).toBe('workflows')
    const index = buildSettingsSearchIndex(pages, t)
    const workflows = index.filter((entry) => entry.page === 'workflows')
    expect(workflows.find((entry) => entry.id === 'workflows-list')).toMatchObject({
      view: 'workflows',
      path: ['工作流']
    })
    expect(workflows.find((entry) => entry.id === 'workflow-background')).toMatchObject({
      view: 'editor',
      prerequisiteId: 'workflows-list'
    })
    expect(
      index
        .filter((entry) => entry.page === 'agentTemplates')
        .some((entry) => entry.id.startsWith('workflow'))
    ).toBe(false)
  })

  it('uses translated titles, descriptions, options and paths with stable ranked matching', () => {
    const pages = [
      {
        id: 'general',
        labelKey: 'settings.page.general' as const,
        nodes: defineSettingsNodes([
          {
            id: 'description',
            title: { text: '其他选项' },
            description: { text: '允许使用 API Token' }
          },
          { id: 'title', title: { text: 'API Token' } },
          { id: 'options', title: { text: '模式' }, terms: [{ text: '自动批准' }] }
        ])
      }
    ]
    const index = buildSettingsSearchIndex(pages, t)
    expect(searchSettings(index, '  ａｐｉ   TOKEN ').map((entry) => entry.id)).toEqual([
      'title',
      'description'
    ])
    expect(searchSettings(index, '常规 自动批准').map((entry) => entry.id)).toEqual(['options'])
    expect(searchSettings(index, '   ')).toEqual([])
    expect(searchSettings(index, '不存在的内容')).toEqual([])
    const en = buildSettingsSearchIndex(pages, (key) => getTranslation('en-US', key))
    expect(en[0].path).toEqual(['General'])
  })

  it('shares platform availability and inherited subview targets with the UI', () => {
    const nodes = defineSettingsNodes([
      { id: 'unavailable', title: { text: '平台专属' }, supported: () => false },
      {
        id: 'parent',
        title: { text: '模型' },
        view: 'model',
        prerequisiteId: 'selector',
        children: [{ id: 'field', title: { text: '上下文' } }]
      }
    ])
    expect(
      renderToStaticMarkup(
        <>
          {renderSettingsNodes(nodes, (node) => (
            <section>{settingLabel(node, t)}</section>
          ))}
        </>
      )
    ).not.toContain('平台专属')
    const results = buildSettingsSearchIndex(
      [{ id: 'configuration', labelKey: 'settings.page.configuration', nodes }],
      t
    )
    expect(results).toHaveLength(1)
    expect(results[0]).toMatchObject({
      id: 'field',
      view: 'model',
      prerequisiteId: 'selector',
      ancestorIds: ['parent'],
      path: ['配置', '模型']
    })
  })

  it('rejects duplicate stable identifiers within a page', () => {
    expect(() =>
      buildSettingsSearchIndex(
        [
          {
            id: 'general',
            labelKey: 'settings.page.general',
            nodes: [
              { id: 'same', title: { text: '一' } },
              { id: 'same', title: { text: '二' } }
            ]
          }
        ],
        t
      )
    ).toThrow('Duplicate setting ID')
  })
})
