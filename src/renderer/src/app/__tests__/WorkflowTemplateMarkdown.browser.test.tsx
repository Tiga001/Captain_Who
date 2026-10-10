import { page, userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  WorkflowRecord,
  WorkflowRequest,
  WorkflowResponse,
  WorkflowTemplateLanguage
} from '@mycopilot/protocol'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { SettingsSearchNavigationProvider } from '../../features/settings/settingsSearchNavigation'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'

const service = vi.hoisted(() => ({
  request: vi.fn(),
  import: vi.fn(),
  export: vi.fn(),
  language: 'zh-CN' as WorkflowTemplateLanguage
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      requestWorkflows: service.request,
      importWorkflowTemplate: service.import,
      exportWorkflowTemplate: service.export
    }
  }
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({ useAccountAuth: () => null }))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [], enabledModels: [] })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: service.language, t: (key: string) => key })
}))
const { WorkflowSettingsSection } = await import('../../features/workflows/WorkflowSettingsSection')

let records: WorkflowRecord[]
function response(): WorkflowResponse {
  return structuredClone({ records, drafts: [], instances: [], issues: [] })
}
function importedRecord(): WorkflowRecord {
  const definition = parseWorkflowDefinition(fixture)
  definition.id = 'imported-template'
  definition.name = '导入的研究组织'
  definition.description = '汇总证据并形成研究报告'
  definition.background = '共同背景\n保留 Markdown **强调** 与中文。'
  definition.departments = [
    { id: 'research', name: '研究部', parentId: null, x: 20, y: 60, width: 870, height: 440 },
    { id: 'methods', name: '方法组', parentId: 'research', x: 40, y: 100, width: 400, height: 300 }
  ]
  definition.nodes = definition.nodes.map((node, index) => ({
    ...node,
    id: `imported-member-${index}`,
    name: ['方法研究员', '研究主管', '组织负责人'][index],
    receives: `输入 ${index}\n原始资料`,
    task: `职责 ${index}\n核验来源`,
    delivers: `交付 ${index}\n完整结论`,
    x: [110, 500, 980][index],
    y: 180,
    rank: [3, 8, 12][index],
    departmentId: ['methods', 'research', null][index],
    managementRole: (['member', 'department_admin', 'organization_admin'] as const)[index],
    permissionMode: 'default' as const,
    modelConfigId: null
  }))
  definition.viewport = { x: 15, y: 25, zoom: 0.85 }
  return {
    definition,
    revision: 1,
    updatedAt: 2,
    enabled: false,
    issues: definition.nodes.map((node) => ({ code: 'node_model', subject: node.id }))
  }
}
function arrangeImport() {
  const imported = importedRecord()
  service.import.mockImplementation(async () => {
    records = [...records, structuredClone(imported)]
    return { ok: true, value: { ...response(), importedTemplateId: imported.definition.id } }
  })
  return imported
}
function view(target: 'workflows' | 'editor' = 'workflows', revision = 1) {
  return (
    <SettingsSearchNavigationProvider
      target={{ page: 'workflows', id: target, view: target, revision }}
    >
      <div style={{ height: 850 }}>
        <WorkflowSettingsSection />
      </div>
    </SettingsSearchNavigationProvider>
  )
}

beforeEach(async () => {
  service.language = 'zh-CN'
  const definition = parseWorkflowDefinition(fixture)
  definition.name = '发布验收'
  records = [{ definition, revision: 7, updatedAt: 1, enabled: true, issues: [] }]
  service.request.mockReset().mockImplementation(async (request: WorkflowRequest) => {
    if (request.operation === 'save') {
      const record = records.find((item) => item.definition.id === request.definition.id)!
      record.definition = structuredClone(request.definition)
      record.revision += 1
    }
    return { ok: true, value: response() }
  })
  service.import.mockReset().mockResolvedValue({ ok: true, value: null })
  service.export.mockReset().mockResolvedValue({ ok: true, value: { saved: false } })
  await page.viewport(1280, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(key, value)
})

describe('organization Markdown template transfer', () => {
  it('keeps import beside create and export beside copy, and silently accepts canceled dialogs', async () => {
    await render(view())
    const importButton = page.getByRole('button', { name: '导入模板', exact: true })
    const createButton = page.getByRole('button', { name: '新建组织模板', exact: true })
    const exportButton = page.getByRole('button', { name: '导出模板 发布验收', exact: true })
    const copyButton = page.getByRole('button', { name: '复制模板 发布验收', exact: true })
    expect(importButton.element().parentElement).toBe(createButton.element().parentElement)
    expect(importButton.element().nextElementSibling).toBe(createButton.element())
    expect(exportButton.element().closest('.workflow-actions')).toBe(
      copyButton.element().closest('.workflow-actions')
    )
    expect(copyButton.element().parentElement?.nextElementSibling).toBe(
      exportButton.element().parentElement
    )
    expect(exportButton.element().className).toBe(copyButton.element().className)
    expect(exportButton.element().textContent).toBe('')
    expect(document.querySelector('.workflow-record')?.querySelectorAll('button')).toHaveLength(4)
    await exportButton.hover()
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('导出模板')
    await userEvent.unhover(exportButton)
    await page.screenshot({
      path: '../../../../../.cache/organization-templates/markdown-library-light.png'
    })
    const changed = vi.fn()
    window.addEventListener('captain:workflows-changed', changed)
    try {
      await importButton.click()
      await expect.poll(() => service.import.mock.calls.length).toBe(1)
      await exportButton.click()
      expect(service.export).toHaveBeenCalledWith({
        id: records[0].definition.id,
        expectedRevision: 7,
        language: 'zh-CN'
      })
      expect(changed).not.toHaveBeenCalled()
      expect(page.getByRole('dialog').query()).toBeNull()
      expect(page.getByRole('alertdialog').query()).toBeNull()
      await expect.element(createButton).toBeVisible()
      expect(service.request).toHaveBeenCalledTimes(1)
      await page.viewport(480, 700)
      const importBounds = importButton.element().getBoundingClientRect()
      const createBounds = createButton.element().getBoundingClientRect()
      expect(importBounds.right).toBeLessThanOrEqual(createBounds.left)
      expect(createBounds.right).toBeLessThanOrEqual(window.innerWidth)
      const card = document.querySelector('.workflow-record')!
      expect(card.scrollWidth).toBeLessThanOrEqual(card.clientWidth)
      await page.screenshot({
        path: '../../../../../.cache/organization-templates/markdown-library-narrow.png'
      })
    } finally {
      window.removeEventListener('captain:workflows-changed', changed)
    }
  })

  it('uses the current interface language on every export and leaves imported authored text intact', async () => {
    const imported = arrangeImport()
    const screen = await render(view())
    await page.getByRole('button', { name: '导入模板', exact: true }).click()
    await page
      .getByRole('alertdialog', { name: '已保存为草稿', exact: true })
      .getByRole('button', { name: '知道了', exact: true })
      .click()
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    await page.getByRole('button', { name: '导出模板 导入的研究组织', exact: true }).click()
    expect(service.export).toHaveBeenLastCalledWith({
      id: imported.definition.id,
      expectedRevision: 1,
      language: 'zh-CN'
    })
    service.language = 'en-US'
    await screen.rerender(view())
    await page.getByRole('button', { name: 'Export template 导入的研究组织', exact: true }).click()
    expect(service.export).toHaveBeenLastCalledWith({
      id: imported.definition.id,
      expectedRevision: 1,
      language: 'en-US'
    })
    expect(records.find((record) => record.definition.id === imported.definition.id)).toEqual(
      imported
    )
    expect(service.request.mock.calls.some(([request]) => request.operation === 'save')).toBe(false)
    await page
      .getByRole('button', { name: 'Edit organization template 导入的研究组织', exact: true })
      .click()
    await expect
      .element(page.getByRole('textbox', { name: 'Organization background', exact: true }))
      .toHaveValue(imported.definition.background)
    await expect
      .element(page.getByRole('textbox', { name: 'Short description', exact: true }))
      .toHaveValue(imported.definition.description)
  })

  it('opens the imported pending template and retains its member configuration in the existing editor', async () => {
    const imported = arrangeImport()
    const changed = vi.fn()
    window.addEventListener('captain:workflows-changed', changed)
    await render(view())
    try {
      await page.getByRole('button', { name: '导入模板', exact: true }).click()
      const warning = page.getByRole('alertdialog', { name: '已保存为草稿', exact: true })
      await expect.element(warning).toHaveTextContent('方法研究员: 请为节点选择模型。')
      await expect.element(warning).toHaveTextContent('研究主管')
      await expect.element(warning).toHaveTextContent('组织负责人')
      expect(changed).toHaveBeenCalledTimes(1)
      expect(warning.element().classList.contains('app-confirm-dialog__card')).toBe(true)
      await warning.getByRole('button', { name: '知道了', exact: true }).click()
      await expect
        .element(page.getByRole('textbox', { name: '名称', exact: true }))
        .toHaveValue(imported.definition.name)
      await expect
        .element(page.getByRole('textbox', { name: '简短描述', exact: true }))
        .toHaveValue(imported.definition.description)
      await expect
        .element(page.getByRole('textbox', { name: '组织公共背景', exact: true }))
        .toHaveValue(imported.definition.background)
      await page.screenshot({
        path: '../../../../../.cache/organization-templates/markdown-imported-editor.png'
      })
      expect(page.getByRole('status', { name: '未保存', exact: true }).query()).toBeNull()
      await page.getByRole('tab', { name: '组织设计', exact: true }).click()
      await userEvent.dblClick(page.getByRole('group', { name: '节点 方法研究员', exact: true }))
      await expect
        .element(page.getByRole('textbox', { name: '这个节点会收到什么', exact: true }))
        .toHaveValue(imported.definition.nodes[0].receives)
      await expect
        .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
        .toHaveValue(imported.definition.nodes[0].task)
      await expect
        .element(page.getByRole('textbox', { name: '这个节点需要交付什么', exact: true }))
        .toHaveValue(imported.definition.nodes[0].delivers)
      await page.getByRole('tab', { name: '职级', exact: true }).click()
      await expect
        .element(page.getByRole('spinbutton', { name: '职级', exact: true }))
        .toHaveValue(3)
      await page.screenshot({
        path: '../../../../../.cache/organization-templates/markdown-imported-structure.png'
      })
      await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
      const saveRequest = service.request.mock.calls
        .map(([request]) => request)
        .find((request) => request.operation === 'save')
      expect(saveRequest).toEqual({
        operation: 'save',
        definition: imported.definition,
        expectedRevision: 1,
        expectedDraftRevision: 0
      })
      await warning.getByRole('button', { name: '知道了', exact: true }).click()
      await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
      await expect
        .element(page.getByRole('button', { name: '导出模板 导入的研究组织', exact: true }))
        .toBeVisible()
      expect(
        records.find((record) => record.definition.id === imported.definition.id)?.enabled
      ).toBe(false)
    } finally {
      window.removeEventListener('captain:workflows-changed', changed)
    }
  })

  it('uses the existing discard dialog before replacing a hidden dirty editor with an import', async () => {
    arrangeImport()
    const screen = await render(view())
    await page.getByRole('button', { name: '编辑组织模板 发布验收', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('尚未保存的名称')
    await screen.rerender(view('workflows', 2))
    await page.getByRole('button', { name: '导入模板', exact: true }).click()
    const discard = page.getByRole('dialog', { name: '放弃未保存的修改？', exact: true })
    await expect.element(discard).toBeVisible()
    expect(page.getByRole('alertdialog').query()).toBeNull()
    await discard.getByRole('button', { name: '继续编辑', exact: true }).last().click()
    await screen.rerender(view('editor', 3))
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('尚未保存的名称')
    await screen.rerender(view('workflows', 4))
    await page.getByRole('button', { name: '编辑组织模板 导入的研究组织', exact: true }).click()
    await discard.getByRole('button', { name: '放弃修改', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('导入的研究组织')
    expect(records[0].definition.name).toBe('发布验收')
    expect(service.import).toHaveBeenCalledTimes(1)
  })

  it('opens the imported template after the user discards the previous editor changes', async () => {
    arrangeImport()
    const screen = await render(view())
    await page.getByRole('button', { name: '编辑组织模板 发布验收', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('尚未保存的名称')
    await screen.rerender(view('workflows', 2))
    await page.getByRole('button', { name: '导入模板', exact: true }).click()
    await page
      .getByRole('dialog', { name: '放弃未保存的修改？', exact: true })
      .getByRole('button', { name: '放弃修改', exact: true })
      .click()
    const warning = page.getByRole('alertdialog', { name: '已保存为草稿', exact: true })
    await expect.element(warning).toHaveTextContent('请为节点选择模型。')
    await warning.getByRole('button', { name: '知道了', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('导入的研究组织')
    expect(records[0].definition.name).toBe('发布验收')
    expect(records).toHaveLength(2)
  })

  it.each([
    ['organization_template_invalid_format', '模板文件格式无效', '符合组织模板格式的 Markdown'],
    ['organization_template_read_failed', '无法读取模板文件', '具有读取权限']
  ])('explains %s without turning it into a library load failure', async (code, title, hint) => {
    service.import.mockResolvedValue({
      ok: false,
      error: { message: code, data: { code, path: '/private/template.md' } }
    })
    await render(view())
    await page.getByRole('button', { name: '导入模板', exact: true }).click()
    const warning = page.getByRole('alertdialog', { name: title, exact: true })
    await expect.element(warning).toHaveTextContent(hint)
    expect(document.body.textContent).not.toContain('/private/template.md')
    expect(document.body.textContent).not.toContain(code)
    await warning.getByRole('button', { name: '知道了', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '编辑组织模板 发布验收', exact: true }))
      .toBeVisible()
    expect(page.getByRole('button', { name: '重试', exact: true }).query()).toBeNull()
  })
})
