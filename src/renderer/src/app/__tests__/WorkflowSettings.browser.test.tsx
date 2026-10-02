import { page, userEvent } from 'vitest/browser'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'
import type { WorkflowRecord, WorkflowRequest, WorkflowIssue } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { parseWorkflowDefinition, validateWorkflowMemberNames } from '@mycopilot/protocol'

const service = vi.hoisted(() => ({
  request: vi.fn(),
  profile: null as import('@mycopilot/host-api').AccountProfile | null,
  models: [] as Array<{
    id: string
    displayName: string
    execution: { status: 'available' } | { status: 'unavailable'; reason: 'disabled' }
  }>
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: service.profile } })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    models: service.models,
    enabledModels: service.models.filter((model) => model.execution.status === 'available')
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) =>
      ({
        'settings.breadcrumb.root': '设置',
        'chat.defaultPermission': '默认权限',
        'chat.customPermission': '自定义权限',
        'chat.fullPermission': '完全权限'
      })[key] ?? '当前位置'
  })
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
const { WorkflowSettingsSection } = await import('../../features/workflows/WorkflowSettingsSection')

function WorkflowTestShell() {
  const [editing, setEditing] = useState(false)
  return (
    <div className="settings-page" data-workflow-editor={editing || undefined}>
      <div className="settings-page__drag-region" />
      <aside className="settings-nav">
        <span>设置</span>
        <span>组织</span>
      </aside>
      <main className={`settings-content${editing ? ' settings-content--workflow-editor' : ''}`}>
        <div className="settings-content__inner">
          <WorkflowSettingsSection onEditorModeChange={setEditing} />
        </div>
      </main>
    </div>
  )
}
function renderWorkflow() {
  return render(<WorkflowTestShell />)
}
async function openStructure() {
  await page.getByRole('tab', { name: '组织设计', exact: true }).click()
}
async function configureNode(name: string) {
  await userEvent.dblClick(page.getByRole('group', { name: `节点 ${name}`, exact: true }))
}
async function addBlank(configure = true) {
  await page.getByRole('button', { name: '添加节点', exact: true }).click()
  await page.getByRole('button', { name: '新建智能体', exact: true }).click()
  expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
  if (configure) await configureNode('未命名节点')
}

let records: WorkflowRecord[]
let validationIssues: WorkflowIssue[]
beforeEach(async () => {
  service.profile = null
  service.models = [
    { id: 'model', displayName: '模型 A', execution: { status: 'available' } },
    { id: 'model-b', displayName: '模型 B', execution: { status: 'available' } },
    {
      id: 'retired-model',
      displayName: '旧模型',
      execution: { status: 'unavailable', reason: 'disabled' }
    }
  ]
  await page.viewport(1440, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(key, value)
  document.documentElement.style.setProperty(
    '--settings-content-text-color',
    'var(--mc-color-text-primary)'
  )
  records = []
  validationIssues = []
  service.request.mockReset().mockImplementation(async (request: WorkflowRequest) => {
    if (request.operation === 'validate') return { records: [], issues: [] }
    if (request.operation === 'save') {
      validateWorkflowMemberNames(request.definition)
      const old = records.find((record) => record.definition.id === request.definition.id)
      if ((old?.revision ?? 0) !== request.expectedRevision)
        throw Object.assign(new Error('Revision conflict'), { code: -32009 })
      records = [
        {
          definition: structuredClone(request.definition),
          revision: request.expectedRevision + 1,
          updatedAt: 1,
          enabled: validationIssues.length === 0,
          issues: structuredClone(validationIssues)
        },
        ...records.filter((record) => record.definition.id !== request.definition.id)
      ]
    }
    if (request.operation === 'delete')
      records = records.filter((record) => record.definition.id !== request.id)
    return structuredClone({ records, issues: [] })
  })
})

describe('native organization editor', () => {
  it.each([
    [
      'organization_duplicate_department_name',
      '同一上级下的部门名称重复',
      '首尾空格和大小写不用于区分'
    ],
    ['organization_department_name_separator', '请修改部门名称', '部门名称不能包含 /']
  ])('explains %s through the shared acknowledgement dialog', async (code, title, hint) => {
    const definition = parseWorkflowDefinition(fixture)
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    const request = service.request.getMockImplementation()!
    service.request.mockImplementation(async (value: WorkflowRequest) => {
      if (value.operation === 'save') throw new Error(`${code}: internal validation details`)
      return request(value)
    })
    await renderWorkflow()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    const warning = page.getByRole('alertdialog', { name: title, exact: true })
    await expect.element(warning).toHaveTextContent(hint)
    expect(document.body.textContent).not.toContain(code)
    expect(document.body.textContent).not.toContain('internal validation details')
    await warning.getByRole('button', { name: '知道了', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '保存组织模板', exact: true }))
      .toBeVisible()
    expect(records[0].revision).toBe(1)
  })

  it('explains duplicate member names and retains the draft until the name is corrected', async () => {
    const definition = parseWorkflowDefinition(fixture)
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await openStructure()
    await page.getByRole('group', { name: `节点 ${definition.nodes[1].name}`, exact: true }).click()
    const name = page.getByRole('textbox', { name: '节点名称', exact: true })
    await name.fill(`\u3000${definition.nodes[0].name.toUpperCase()} `)
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    const warning = page.getByRole('alertdialog', { name: '成员名称重复', exact: true })
    await expect.element(warning).toHaveTextContent('不同部门的成员也不能重名')
    expect(document.body.textContent).not.toContain('organization_duplicate_member_name')
    expect(records[0].revision).toBe(1)
    await warning.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(name).toHaveValue(`\u3000${definition.nodes[0].name.toUpperCase()} `)
    await name.fill('Reviewer')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.nodes[1].name).toBe('Reviewer')
  })

  it('copies node settings through native clipboard events, pastes repeatedly and supports undo', async () => {
    const definition = parseWorkflowDefinition(fixture)
    const original = definition.nodes.find((node) => node.kind === 'agent')!
    if (original.kind !== 'agent') throw new Error('Expected agent')
    Object.assign(original, {
      permissionMode: 'full',
      modelConfigId: 'model-b',
      receives: '原始文案',
      task: '润色文案',
      delivers: '交付文案'
    })
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await openStructure()
    await page.getByRole('group', { name: `节点 ${original.name}`, exact: true }).click()
    const clipboard = new DataTransfer()
    const copy = new ClipboardEvent('copy', {
      bubbles: true,
      cancelable: true,
      clipboardData: clipboard
    })
    document.activeElement!.dispatchEvent(copy)
    expect(copy.defaultPrevented).toBe(true)
    expect(clipboard.getData('text/plain')).toContain('润色文案')
    const paste = () => {
      const event = new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData: clipboard
      })
      document.querySelector('.workflow-canvas')!.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(true)
    }
    const count = () => document.querySelectorAll('[data-workflow-node-id]').length
    paste()
    await expect.poll(count).toBe(definition.nodes.length + 1)
    paste()
    await expect.poll(count).toBe(definition.nodes.length + 2)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect.poll(count).toBe(definition.nodes.length + 1)
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect.poll(count).toBe(definition.nodes.length + 2)
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    const saved = records[0].definition
    const copies = saved.nodes.slice(definition.nodes.length)
    expect(new Set(saved.nodes.map((node) => node.id)).size).toBe(saved.nodes.length)
    for (const [index, node] of copies.entries()) {
      expect(node).toEqual({
        ...original,
        name: `${original.name} (${index + 2})`,
        id: node.id,
        x: node.x,
        y: node.y
      })
      expect({ x: node.x, y: node.y }).not.toEqual({ x: original.x, y: original.y })
    }
    expect(copies[0].x !== copies[1].x || copies[0].y !== copies[1].y).toBe(true)
    expect(saved).not.toHaveProperty('flows')

    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('另一个模板')
    await openStructure()
    clipboard.clearData('application/x-captain-workflow-node')
    paste()
    await expect.poll(count).toBe(1)
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].definition.name).toBe('另一个模板')
    expect(records[0].definition.nodes[0]).toMatchObject({
      name: original.name,
      modelConfigId: 'model-b',
      permissionMode: 'full',
      receives: '原始文案',
      task: '润色文案',
      delivers: '交付文案'
    })
    expect(records[0].definition).not.toHaveProperty('flows')
  })

  it('leaves text editing clipboard actions alone and ignores ordinary pasted text', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await openStructure()
    await addBlank()
    const clipboard = new DataTransfer()
    clipboard.setData('text/plain', 'ordinary text')
    const input = document.querySelector('.workflow-graph-inspector textarea')!
    for (const type of ['copy', 'paste']) {
      const event = new ClipboardEvent(type, {
        bubbles: true,
        cancelable: true,
        clipboardData: clipboard
      })
      input.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(false)
    }
    const event = new ClipboardEvent('paste', {
      bubbles: true,
      cancelable: true,
      clipboardData: clipboard
    })
    document.querySelector('.workflow-canvas')!.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    expect(document.querySelectorAll('[data-workflow-node-id]')).toHaveLength(1)
  })

  it('lists valid templates and incomplete drafts without any template activation switches', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.name = '版本发布验收'
    records = [
      { definition, revision: 1, updatedAt: 1, enabled: true, issues: [] },
      {
        definition: { ...structuredClone(definition), id: 'draft', name: '未完成流程' },
        revision: 1,
        updatedAt: 1,
        enabled: false,
        issues: [{ code: 'task', subject: '' }]
      }
    ]
    const view = await renderWorkflow()
    await expect
      .element(page.getByRole('button', { name: '编辑组织模板 版本发布验收', exact: true }))
      .toBeVisible()
    await expect
      .element(page.getByRole('button', { name: '编辑组织模板 未完成流程', exact: true }))
      .toBeVisible()
    await expect.element(page.getByText('草稿', { exact: true })).toBeVisible()
    expect(page.getByRole('switch').query()).toBeNull()
    expect(document.body.textContent).not.toContain('图结构已校验')
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual(['list'])
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-templates-availability-light.png'
    })
    await view.unmount()
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    await renderWorkflow()
    await expect
      .element(page.getByRole('button', { name: '编辑组织模板 版本发布验收', exact: true }))
      .toBeVisible()
    expect(page.getByRole('switch').query()).toBeNull()
    expect(records[0].enabled).toBe(true)
    expect(records[1].enabled).toBe(false)
    expect(records.map((record) => record.revision)).toEqual([1, 1])
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-templates-availability-dark.png'
    })
  })

  it('makes an invalid saved draft unavailable and automatically restores availability when corrected', async () => {
    const definition = parseWorkflowDefinition(fixture)
    records = [{ definition, revision: 1, updatedAt: 1, enabled: true, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑组织模板 Develop and review', exact: true }).click()
    validationIssues = [{ code: 'name', subject: '' }]
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].enabled).toBe(false)
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    expect(page.getByRole('switch').query()).toBeNull()
    await expect.element(page.getByText('草稿', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '编辑组织模板', exact: true }).click()
    validationIssues = []
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('版本发布验收')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    expect(page.getByRole('switch').query()).toBeNull()
    expect(records[0].enabled).toBe(true)
    expect(page.getByText('草稿', { exact: true }).query()).toBeNull()
    expect(document.body.textContent).not.toContain('图结构已校验')
  })

  it('offers only agents and departments and ignores user-node creation payloads', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await openStructure()
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    const picker = page.getByRole('dialog', { name: '添加节点', exact: true })
    await expect
      .element(picker.getByRole('button', { name: '新建智能体', exact: true }))
      .toBeVisible()
    await expect
      .element(picker.getByRole('button', { name: '添加部门', exact: true }))
      .toBeVisible()
    expect(picker.getByRole('button', { name: '用户', exact: true }).query()).toBeNull()
    const canvasElement = document.querySelector('.workflow-canvas')!
    const clipboard = new DataTransfer()
    clipboard.setData('application/x-captain-workflow-template', 'node:user')
    clipboard.setData(
      'text/plain',
      JSON.stringify({
        type: 'application/x-captain-workflow-node',
        version: 1,
        node: { id: 'user', kind: 'user', name: '用户', task: '', x: 100, y: 100 }
      })
    )
    canvasElement.dispatchEvent(
      new DragEvent('drop', {
        bubbles: true,
        cancelable: true,
        dataTransfer: clipboard
      })
    )
    canvasElement.dispatchEvent(
      new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData: clipboard
      })
    )
    expect(document.querySelectorAll('[data-workflow-node-id]')).toHaveLength(0)
    await picker
      .getByRole('button', { name: '新建智能体', exact: true })
      .dropTo(page.elementLocator(canvasElement), { targetPosition: { x: 440, y: 240 } })
    await expect
      .element(page.getByRole('group', { name: '节点 未命名节点', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    expect(records[0].definition.nodes).toHaveLength(1)
    expect(records[0].definition.nodes[0].kind).toBe('agent')
  })

  it('shares metadata and graph across tabs and persists the whole organization with one save', async () => {
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveAttribute('placeholder', '例如：产品方案评审、版本发布验收')
    await expect
      .element(page.getByRole('textbox', { name: '简短描述', exact: true }))
      .toHaveAttribute('placeholder', '说明这个组织适合处理什么任务、何时使用')
    await expect
      .element(page.getByRole('textbox', { name: '组织公共背景', exact: true }))
      .toHaveAttribute('placeholder', '填写所有节点都需要了解的目标、背景和共同要求')
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-details.png'
    })
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('验证码开发')
    await page.getByRole('textbox', { name: '简短描述', exact: true }).fill('分析、开发和审查')
    await page.getByRole('textbox', { name: '组织公共背景', exact: true }).fill('实现登录功能')
    await openStructure()
    expect(document.querySelector('.workflow-modal-backdrop')).toBeNull()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await addBlank()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('开发')
    await page.getByRole('button', { name: '选择模型', exact: true }).click()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 开发', exact: true }))
      .toHaveTextContent('模型 B')
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('实现验证码登录并验证。')
    await new Promise((resolve) => window.setTimeout(resolve, 450))
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual(['list'])
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '组织公共背景', exact: true }))
      .toHaveValue('实现登录功能')
    await openStructure()
    await expect.element(page.getByRole('group', { name: '节点 开发', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    expect(records[0].definition.nodes[0]).toMatchObject({
      permissionMode: 'default',
      modelConfigId: 'model-b',
      task: '实现验证码登录并验证。'
    })
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual([
      'list',
      'save'
    ])
    await expect.element(page.getByRole('tab', { name: '组织设计', exact: true })).toBeVisible()
    await expect
      .element(page.getByRole('status', { name: '未保存', exact: true }))
      .not.toBeInTheDocument()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-node.png'
    })
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    expect(document.body.textContent).not.toMatch(
      /定义校验|工作流已保存|可复用的协作流程|当前可设计和保存工作流模板|个节点|条连线/
    )
    await view.unmount()
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑组织模板 验证码开发', exact: true }).click()
    await openStructure()
    await expect.element(page.getByRole('group', { name: '节点 开发', exact: true })).toBeVisible()
    await expect
      .element(page.getByRole('group', { name: '节点 开发', exact: true }))
      .toHaveTextContent('模型 B')
  })

  it('edits conversation permissions independently of models and preserves them after reopening', async () => {
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    let view = await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('独立对话协作')
    await openStructure()
    await addBlank(false)
    const permissionPicker = page.getByRole('button', { name: '权限', exact: true })
    await expect.element(permissionPicker).toHaveTextContent('默认权限')
    expect(permissionPicker.element().querySelector('.lucide-shield-plus')).not.toBeNull()
    expect(
      page.getByRole('button', { name: '选择模型', exact: true }).element().getBoundingClientRect()
        .left
    ).toBeGreaterThan(permissionPicker.element().getBoundingClientRect().right)
    expect(document.body.textContent).not.toContain('子智能体')
    await page.getByRole('button', { name: '选择模型', exact: true }).click()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    for (const [label, mode, icon] of [
      ['自定义权限', 'custom', 'shield-check'],
      ['完全权限', 'full', 'shield-alert'],
      ['默认权限', 'default', 'shield-plus']
    ]) {
      await permissionPicker.click()
      expect(document.querySelectorAll('[role="option"]')).toHaveLength(3)
      expect(
        page
          .getByRole('option', { name: label, exact: true })
          .element()
          .querySelector(`.lucide-${icon}`)
      ).not.toBeNull()
      await page.screenshot({
        path: '../../../../../.cache/workflow-authoring/conversation-permissions-dark.png'
      })
      await page.getByRole('option', { name: label, exact: true }).click()
      await expect.element(permissionPicker).toHaveFocus()
      await expect.element(permissionPicker).toHaveTextContent(label)
      await expect
        .element(page.getByRole('button', { name: '选择模型', exact: true }))
        .toHaveTextContent('模型 B')
      await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
      await expect
        .poll(() => records[0]?.definition.nodes[0])
        .toMatchObject({ permissionMode: mode, modelConfigId: 'model-b' })
      expect(records[0].definition.nodes[0]).not.toHaveProperty('templateId')
    }
    await permissionPicker.click()
    await userEvent.keyboard('{End}{Enter}')
    await expect.element(permissionPicker).toHaveTextContent('自定义权限')
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect.element(permissionPicker).toHaveTextContent('默认权限')
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect.element(permissionPicker).toHaveTextContent('自定义权限')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect
      .poll(() => records[0]?.definition.nodes[0])
      .toMatchObject({ permissionMode: 'custom' })
    await view.unmount()
    view = await renderWorkflow()
    await page.getByRole('button', { name: '编辑组织模板 独立对话协作', exact: true }).click()
    await openStructure()
    await page.getByRole('group', { name: '节点 未命名节点', exact: true }).click()
    await expect.element(permissionPicker).toHaveTextContent('自定义权限')
    await expect
      .element(page.getByRole('button', { name: '选择模型', exact: true }))
      .toHaveTextContent('模型 B')
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await permissionPicker.click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/conversation-permissions-light.png'
    })
    await view.unmount()
  })

  it('keeps an unavailable saved model until reselected and allows an unconfigured draft', async () => {
    const definition = parseWorkflowDefinition(fixture)
    if (definition.nodes[0].kind !== 'agent') throw new Error('Expected agent')
    definition.nodes[0].modelConfigId = 'retired-model'
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '编辑组织模板 Develop and review', exact: true }).click()
    await openStructure()
    await configureNode('implement')
    const modelPicker = page.getByRole('button', { name: '选择模型', exact: true })
    await expect.element(modelPicker).toHaveTextContent('旧模型 · 模型不可用')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.nodes[0]).toMatchObject({ modelConfigId: 'retired-model' })
    await modelPicker.click()
    await expect
      .element(page.getByRole('option', { name: '旧模型 · 模型不可用', exact: true }))
      .toBeDisabled()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 implement', exact: true }))
      .toHaveTextContent('模型 B')
    await view.unmount()
    service.models = []
    records = []
    await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await openStructure()
    await addBlank()
    await expect.element(modelPicker).toBeDisabled()
    await expect.element(modelPicker).toHaveTextContent('暂无可用模型')
    await expect
      .element(page.getByRole('group', { name: '节点 未命名节点', exact: true }))
      .toHaveTextContent('未选择模型')
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0]?.definition.nodes[0]).toMatchObject({ modelConfigId: null })
  })

  it('shows save issues only in an acknowledgement dialog and leaves the saved editor ready', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建组织模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('草稿流程')
    validationIssues = [
      { code: 'empty', subject: '' },
      { code: 'task', subject: '' },
      { code: 'task', subject: '' }
    ]
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    const dialog = page.getByRole('alertdialog', { name: '已保存为草稿', exact: true })
    await expect.element(dialog).toHaveTextContent('请添加至少一个智能体节点。')
    await dialog.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(dialog).not.toBeInTheDocument()
    await expect
      .element(page.getByRole('button', { name: '保存组织模板', exact: true }))
      .toHaveFocus()
    expect(document.body.textContent).not.toContain('请添加至少一个智能体节点。')
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual([
      'list',
      'save'
    ])
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '编辑组织模板 草稿流程', exact: true }))
      .toBeVisible()
  })
})
