import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  WorkflowRecord,
  WorkflowRequest,
  WorkflowEditingDraft,
  WorkflowInvalidRecord,
  WorkflowInvalidDraft,
  WorkflowTemplateUsage
} from '@mycopilot/protocol'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { SettingsSearchNavigationProvider } from '../../features/settings/settingsSearchNavigation'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'

const service = vi.hoisted(() => ({ request: vi.fn() }))
vi.mock('../../features/auth/AccountAuthContext', () => ({ useAccountAuth: () => null }))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [], enabledModels: [] })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', t: (key: string) => key })
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
const { WorkflowSettingsSection } = await import('../../features/workflows/WorkflowSettingsSection')

let records: WorkflowRecord[]
let drafts: WorkflowEditingDraft[]
let invalidRecords: WorkflowInvalidRecord[]
let invalidDrafts: WorkflowInvalidDraft[]
let usages: WorkflowTemplateUsage[]
let startRunningOnSave: boolean
beforeEach(async () => {
  const definition = parseWorkflowDefinition(fixture)
  definition.name = '发布验收'
  records = [{ definition, revision: 1, updatedAt: 1, enabled: true, issues: [] }]
  drafts = []
  invalidRecords = []
  invalidDrafts = []
  usages = []
  startRunningOnSave = false
  await page.viewport(1280, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(key, value)
  service.request.mockReset().mockImplementation(async (request: WorkflowRequest) => {
    if (request.operation === 'save') {
      const old = records.find((record) => record.definition.id === request.definition.id)
      const usage = usages.find((item) => item.templateId === request.definition.id)
      if (startRunningOnSave && usage) {
        usage.instances[0].running = true
        usage.usageRevision = 'running'
        startRunningOnSave = false
      }
      if (usage?.instances.some((item) => item.running))
        throw Object.assign(new Error('workflow_template_running'), { code: -32009 })
      if (usage?.instances.length && request.expectedUsageRevision !== usage.usageRevision)
        throw Object.assign(new Error('workflow_usage_changed'), { code: -32009 })
      if ((old?.revision ?? 0) !== request.expectedRevision)
        throw Object.assign(new Error('Revision conflict'), { code: -32009 })
      const storedDraft = drafts.find((item) => item.definition.id === request.definition.id)
      if ((storedDraft?.revision ?? 0) !== (request.expectedDraftRevision ?? 0))
        throw Object.assign(new Error('workflow_draft_changed'), { code: -32009 })
      records = [
        {
          definition: structuredClone(request.definition),
          revision: request.expectedRevision + 1,
          updatedAt: 1,
          enabled: true,
          issues: []
        },
        ...records.filter((record) => record.definition.id !== request.definition.id)
      ]
      drafts = drafts.filter((item) => item.definition.id !== request.definition.id)
    }
    if (request.operation === 'saveDraft') {
      const old = drafts.find((item) => item.definition.id === request.definition.id)
      expect(request.expectedDraftRevision).toBe(old?.revision ?? 0)
      drafts = [
        {
          definition: structuredClone(request.definition),
          baseRevision: request.expectedRevision,
          revision: request.expectedDraftRevision + 1,
          updatedAt: 2
        },
        ...drafts.filter((item) => item.definition.id !== request.definition.id)
      ]
    }
    if (request.operation === 'delete') {
      if (usages.some((item) => item.templateId === request.id && item.instances.length > 0)) {
        throw Object.assign(new Error('workflow_template_in_use'), { code: -32009 })
      }
      const stored =
        records.find((item) => item.definition.id === request.id) ??
        invalidRecords.find((item) => item.id === request.id)
      expect(request.expectedRevision).toBe(stored?.revision)
      records = records.filter((item) => item.definition.id !== request.id)
      invalidRecords = invalidRecords.filter((item) => item.id !== request.id)
    }
    if (request.operation === 'deleteDraft') {
      const stored =
        drafts.find((item) => item.definition.id === request.id) ??
        invalidDrafts.find((item) => item.id === request.id)
      expect(request.expectedDraftRevision).toBe(stored?.revision)
      drafts = drafts.filter((item) => item.definition.id !== request.id)
      invalidDrafts = invalidDrafts.filter((item) => item.id !== request.id)
    }
    if (request.operation === 'duplicate') {
      const original = records.find((item) => item.definition.id === request.id)!
      records.push({
        ...structuredClone(original),
        definition: {
          ...structuredClone(original.definition),
          id: request.newId,
          name: request.name
        },
        revision: 1,
        enabled: original.issues.length === 0
      })
    }
    return structuredClone({ records, drafts, usages, invalidRecords, invalidDrafts, issues: [] })
  })
})

function useTemplate(running = false) {
  usages = [
    {
      templateId: records[0].definition.id,
      usageRevision: 'usage-1',
      instances: [
        { id: 'instance-1', name: '功能验收', running, projectNames: ['桌面端', '服务端'] },
        { id: 'instance-2', name: '独立调研', running: false, projectNames: [] }
      ]
    }
  ]
}
async function openEditor() {
  await page.getByRole('button', { name: '编辑工作流模板 发布验收', exact: true }).click()
}
async function editAndSave(name = '改进后的验收') {
  await page.getByRole('textbox', { name: '名称', exact: true }).fill(name)
  await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
}

function view() {
  return render(<WorkflowSettingsSection />)
}

describe('workflow template lifecycle', () => {
  it('isolates unavailable templates while valid templates and new saves remain usable', async () => {
    invalidRecords = [
      {
        id: 'legacy-template',
        name: '1234',
        revision: 7,
        updatedAt: 1,
        reason: 'incompatible_definition'
      }
    ]
    await view()
    const unavailable = page.getByRole('article', { name: '模板不可用 1234', exact: true })
    await expect.element(unavailable).toHaveTextContent('旧格式已不再支持，不会自动转换')
    expect(unavailable.getByRole('button').all()).toHaveLength(1)
    expect(unavailable.getByRole('switch').query()).toBeNull()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 发布验收', exact: true }))
      .toBeVisible()
    expect(page.getByRole('alert').query()).toBeNull()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await editAndSave('新的有效模板')
    await expect.poll(() => records.length).toBe(2)
    expect(page.getByRole('alert').query()).toBeNull()
    expect(records[0].enabled).toBe(true)
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    await expect.element(unavailable).toBeVisible()
    await unavailable.getByRole('button', { name: '删除工作流模板 1234', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '删除这个工作流？', exact: true })
    await expect.element(confirmation).toHaveTextContent('1234')
    await confirmation.getByRole('button', { name: '取消', exact: true }).last().click()
    expect(invalidRecords).toHaveLength(1)
    expect(service.request.mock.calls.some(([request]) => request.operation === 'delete')).toBe(
      false
    )
    await unavailable.getByRole('button', { name: '删除工作流模板 1234', exact: true }).click()
    await confirmation.getByRole('button', { name: '删除工作流模板', exact: true }).click()
    await expect.poll(() => invalidRecords.length).toBe(0)
    await expect.element(unavailable).not.toBeInTheDocument()
    expect(records).toHaveLength(2)
    expect(service.request).toHaveBeenLastCalledWith({
      operation: 'delete',
      id: 'legacy-template',
      expectedRevision: 7
    })
  })

  it('keeps a published template usable and discards only its unavailable editing draft after confirmation', async () => {
    const original = structuredClone(records[0])
    invalidDrafts = [
      {
        id: original.definition.id,
        name: '旧的暂存方案',
        baseRevision: 1,
        revision: 4,
        updatedAt: 1,
        reason: 'incompatible_definition'
      }
    ]
    await view()
    const unavailable = page.getByRole('article', {
      name: '暂存修改不可用 旧的暂存方案',
      exact: true
    })
    await expect.element(unavailable).toHaveTextContent('删除暂存修改不会影响正式模板')
    expect(unavailable.getByRole('button').all()).toHaveLength(1)
    await openEditor()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('发布验收')
    await expect.element(page.getByRole('status')).toHaveTextContent('当前编辑的是正式模板')
    await expect
      .element(page.getByRole('button', { name: '保存工作流模板', exact: true }))
      .toBeDisabled()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('保留当前编辑')
    await page.getByRole('button', { name: '删除暂存修改', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '删除这份暂存修改？', exact: true })
    await expect.element(confirmation).toHaveTextContent('正式模板及其工作流保持不变')
    await confirmation.getByRole('button', { name: '取消', exact: true }).last().click()
    expect(invalidDrafts).toHaveLength(1)
    await page.getByRole('button', { name: '删除暂存修改', exact: true }).click()
    await confirmation.getByRole('button', { name: '删除暂存修改', exact: true }).click()
    await expect.poll(() => invalidDrafts.length).toBe(0)
    expect(records[0]).toEqual(original)
    expect(service.request).toHaveBeenLastCalledWith({
      operation: 'deleteDraft',
      id: original.definition.id,
      expectedDraftRevision: 4
    })
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('保留当前编辑')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.name).toBe('保留当前编辑')
    expect(page.getByRole('alert').query()).toBeNull()
  })

  it('refreshes unavailable records after a successful template copy', async () => {
    invalidRecords = [
      {
        id: 'invalid-template',
        name: '无效模板',
        revision: 2,
        updatedAt: 1,
        reason: 'invalid_definition'
      }
    ]
    await view()
    const unavailable = page.getByRole('article', { name: '模板不可用 无效模板', exact: true })
    await expect.element(unavailable).toHaveTextContent('数据无效')
    invalidRecords = []
    await page.getByRole('button', { name: '复制模板 发布验收', exact: true }).click()
    await expect.element(unavailable).not.toBeInTheDocument()
  })

  it('explains a template still in use with an acknowledgement dialog instead of a page error', async () => {
    useTemplate()
    await view()
    const title = page.getByRole('heading', { name: '工作流模板', exact: true })
    expect(
      getComputedStyle(title.element().closest('section')!).marginTop
    ).toBe('0px')
    await page.getByRole('button', { name: '删除工作流模板 发布验收', exact: true }).click()
    await page
      .getByRole('dialog', { name: '删除这个工作流？', exact: true })
      .getByRole('button', { name: '删除工作流模板', exact: true })
      .click()
    const warning = page.getByRole('alertdialog', { name: '无法删除工作流模板', exact: true })
    await expect.element(warning).toHaveTextContent('功能验收')
    await expect.element(warning).toHaveTextContent('独立调研')
    expect(page.getByRole('alert').query()).toBeNull()
    expect(records).toHaveLength(1)
    await page.screenshot({
      element: warning.element() as HTMLElement,
      path: '../../../../../.cache/workflow-authoring/template-in-use-warning.png'
    })
    await warning.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(warning).not.toBeInTheDocument()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 发布验收', exact: true }))
      .toBeVisible()
    expect(page.getByRole('alert').query()).toBeNull()
  })

  it('shows returned load diagnostics and clears them after a successful retry', async () => {
    service.request.mockRejectedValueOnce(
      Object.assign(new Error('storage_unavailable'), {
        code: -32000,
        data: { secret: 'hidden detail' }
      })
    )
    await view()
    await expect.element(page.getByRole('alert')).toHaveTextContent('工作流加载或保存失败')
    await page.getByText('错误详情', { exact: true }).click()
    await expect.element(page.getByRole('alert')).toHaveTextContent('[-32000] storage_unavailable')
    expect(document.body.textContent).not.toContain('hidden detail')
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 发布验收', exact: true }))
      .toBeVisible()
    expect(page.getByRole('alert').query()).toBeNull()
  })

  it('retries an initial library failure from the new editor without losing entered content', async () => {
    service.request.mockRejectedValueOnce(
      Object.assign(new Error('storage_unavailable'), { code: -32000 })
    )
    await view()
    await expect.element(page.getByRole('alert')).toHaveTextContent('工作流加载或保存失败')
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('重试时保留名称')
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await expect.element(page.getByRole('alert')).not.toBeInTheDocument()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('重试时保留名称')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(2)
    expect(records[0].definition.name).toBe('重试时保留名称')
  })

  it('duplicates the published template without touching its draft or instances', async () => {
    useTemplate(true)
    const original = structuredClone(records[0])
    drafts = [
      {
        definition: { ...structuredClone(original.definition), name: '未发布修改' },
        baseRevision: 1,
        revision: 1,
        updatedAt: 1
      }
    ]
    await view()
    await page.getByRole('button', { name: '复制模板 发布验收', exact: true }).click()
    await expect.poll(() => records.length).toBe(2)
    expect(records[0]).toEqual(original)
    expect(records[1].definition.name).toBe('发布验收 (副本)')
    expect(records[1].definition.id).not.toBe(original.definition.id)
    expect(records[1].enabled).toBe(true)
    expect(drafts[0].definition.name).toBe('未发布修改')
    expect(usages[0].templateId).toBe(original.definition.id)
  })

  it('lists instances and their projects and requires confirmation before publishing', async () => {
    useTemplate()
    await view()
    await openEditor()
    await editAndSave()
    const dialog = page.getByRole('dialog', { name: '更新正在使用的模板？', exact: true })
    await expect.element(dialog).toHaveTextContent('功能验收')
    await expect.element(dialog).toHaveTextContent('桌面端 · 服务端')
    await expect.element(dialog).toHaveTextContent('独立调研')
    expect(records[0].definition.name).toBe('发布验收')
    await dialog.getByRole('button', { name: '继续编辑', exact: true }).click()
    expect(records[0].revision).toBe(1)
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await dialog.getByRole('button', { name: '保存并要求重新确认', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(service.request).toHaveBeenLastCalledWith(
      expect.objectContaining({
        operation: 'save',
        expectedUsageRevision: 'usage-1'
      })
    )
  })

  it('stashes changes while running and restores them after reopening without publishing', async () => {
    useTemplate(true)
    const initial = structuredClone(records[0])
    const current = await view()
    await openEditor()
    await editAndSave()
    const dialog = page.getByRole('dialog', { name: '此模板有正在运行的工作流', exact: true })
    await expect.element(dialog).toBeVisible()
    expect(
      dialog.getByRole('button', { name: '保存并要求重新确认', exact: true }).query()
    ).toBeNull()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/template-running-guard.png'
    })
    await dialog.getByRole('button', { name: '暂存修改', exact: true }).click()
    await expect.element(page.getByRole('status')).toHaveTextContent('修改已暂存')
    expect(records[0]).toEqual(initial)
    expect(drafts[0].definition.name).toBe('改进后的验收')
    await current.unmount()
    await view()
    await expect.element(page.getByText('有暂存修改', { exact: true })).toBeVisible()
    await openEditor()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('改进后的验收')
    await expect.element(page.getByRole('status')).toHaveTextContent('已恢复暂存修改')
    usages[0].instances[0].running = false
    usages[0].usageRevision = 'stopped'
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await page.getByRole('button', { name: '保存并要求重新确认', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(drafts).toHaveLength(0)
  })

  it('saves edited content as an independent copy while the original is running', async () => {
    useTemplate(true)
    const original = structuredClone(records[0])
    await view()
    await openEditor()
    await editAndSave()
    await page.getByRole('button', { name: '保存为副本', exact: true }).click()
    await expect.poll(() => records.length).toBe(2)
    expect(records.find((item) => item.definition.id === original.definition.id)).toEqual(original)
    expect(records[0].definition.name).toBe('改进后的验收 (副本)')
    expect(usages[0].templateId).toBe(original.definition.id)
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('改进后的验收 (副本)')
  })

  it('rechecks server conflicts if an instance starts after the usage confirmation', async () => {
    useTemplate()
    await view()
    await openEditor()
    await editAndSave()
    startRunningOnSave = true
    await page.getByRole('button', { name: '保存并要求重新确认', exact: true }).click()
    await expect
      .element(page.getByRole('dialog', { name: '此模板有正在运行的工作流', exact: true }))
      .toBeVisible()
    expect(records[0].definition.name).toBe('发布验收')
    await page.getByRole('button', { name: '暂存修改', exact: true }).click()
    await expect.poll(() => drafts.length).toBe(1)
  })

  it('opens a normal new editor from a settings navigation target', async () => {
    await render(
      <SettingsSearchNavigationProvider
        target={{ page: 'workflows', id: 'workflow-create', view: 'create', revision: 1 }}
      >
        <WorkflowSettingsSection />
      </SettingsSearchNavigationProvider>
    )
    await expect.element(page.getByRole('textbox', { name: '名称', exact: true })).toHaveValue('')
    await expect
      .element(page.getByRole('button', { name: '返回工作流模板列表', exact: true }))
      .toBeVisible()
    expect(document.body.textContent).not.toContain('返回项目')
  })

  it('preserves a newer editing draft when an older editor tries to publish', async () => {
    const original = structuredClone(records[0])
    drafts = [
      {
        definition: { ...structuredClone(original.definition), name: '原暂存修改' },
        baseRevision: 1,
        revision: 1,
        updatedAt: 1
      }
    ]
    await view()
    await openEditor()
    drafts[0] = {
      ...drafts[0],
      definition: { ...drafts[0].definition, name: '另一窗口的新修改' },
      revision: 2
    }
    await editAndSave('当前窗口修改')
    await expect.element(page.getByRole('alert')).toBeVisible()
    expect(records[0]).toEqual(original)
    expect(drafts[0].definition.name).toBe('另一窗口的新修改')
    await page.getByRole('button', { name: '保存为副本', exact: true }).click()
    await expect.poll(() => records.length).toBe(2)
    expect(records[0].definition.name).toBe('当前窗口修改 (副本)')
    expect(drafts[0].definition.name).toBe('另一窗口的新修改')
  })
})
