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
import { parseWorkflowDefinition } from '@mycopilot/protocol'

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
vi.mock('../../features/workflows/workflowClient', () => ({
  requestWorkflows: service.request,
  importWorkflowTemplate: vi.fn(),
  exportWorkflowTemplate: vi.fn()
}))
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

function pointer(element: Element, type: string, x: number, y: number) {
  element.dispatchEvent(
    new PointerEvent(type, {
      bubbles: true,
      cancelable: true,
      pointerId: 1,
      pointerType: 'mouse',
      button: 0,
      buttons: type === 'pointerup' ? 0 : 1,
      clientX: x,
      clientY: y
    })
  )
}

async function drawDepartment(x: number, y: number, width: number, height: number) {
  await page.getByRole('button', { name: '添加节点', exact: true }).click()
  await page.getByRole('button', { name: '添加部门', exact: true }).click()
  const canvas = document.querySelector<HTMLElement>('.workflow-canvas')!
  const stage = document.querySelector<HTMLElement>('.workflow-canvas__stage')!
  const rect = stage.getBoundingClientRect()
  const capture = vi.spyOn(canvas, 'setPointerCapture').mockImplementation(() => undefined)
  // These fixtures start at positive coordinates with the editor's default origin.
  const left = rect.left + 256 + x
  const top = rect.top + 160 + y
  pointer(canvas, 'pointerdown', left, top)
  pointer(canvas, 'pointermove', left + width, top + height)
  pointer(canvas, 'pointerup', left + width, top + height)
  capture.mockRestore()
}

describe('organization template department authoring', () => {
  it('draws nested departments, assigns members, and saves/restores the full template', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes = definition.nodes.filter((node) => node.kind === 'agent').slice(0, 2)
    expect(definition.nodes).toHaveLength(2)
    definition.nodes.forEach((node, index) =>
      Object.assign(node, {
        x: 100,
        y: index === 0 ? 100 : 260,
        rank: index === 0 ? 8 : 3,
        managementRole: index === 0 ? 'organization_admin' : 'member'
      })
    )
    definition.viewport = { x: 0, y: 0, zoom: 1 }
    records = [{ definition, revision: 1, updatedAt: 1, enabled: true, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await openStructure()
    await drawDepartment(40, 40, 580, 400)
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(1)
    await page.getByRole('textbox', { name: '部门名称', exact: true }).fill('研究部')
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    expect(document.querySelector('[aria-label="上级部门"]')).toHaveTextContent('直属组织')
    await drawDepartment(70, 220, 340, 130)
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(2)
    await page.getByRole('textbox', { name: '部门名称', exact: true }).fill('分析组')
    expect(document.querySelector('[aria-label="上级部门"]')).toHaveTextContent('研究部')
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await expect
      .element(page.getByRole('button', { name: '分析组 · 2 级部门', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    const saved = parseWorkflowDefinition(records[0].definition)
    const parent = saved.departments!.find((department) => department.name === '研究部')!
    const child = saved.departments!.find((department) => department.name === '分析组')!
    expect(child.parentId).toBe(parent.id)
    expect(parent.parentId).toBeNull()
    expect(saved.nodes[0]).toMatchObject({
      rank: 8,
      managementRole: 'organization_admin',
      departmentId: parent.id
    })
    expect(saved.nodes[1]).toMatchObject({
      rank: 3,
      managementRole: 'member',
      departmentId: child.id
    })
    await page.getByRole('button', { name: '返回组织模板列表', exact: true }).click()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await openStructure()
    await expect
      .element(page.getByRole('button', { name: '研究部 · 1 级部门', exact: true }))
      .toBeVisible()
    await expect
      .element(page.getByRole('button', { name: '分析组 · 2 级部门', exact: true }))
      .toBeVisible()
    await userEvent.dblClick(
      page.getByRole('group', { name: `节点 ${definition.nodes[1].name}`, exact: true })
    )
    await page.getByRole('tab', { name: '职级', exact: true }).click()
    await expect.element(page.getByText('所属部门', { exact: true })).not.toBeInTheDocument()
    await page.getByRole('button', { name: '分析组 · 2 级部门', exact: true }).click()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    expect(document.querySelector('[aria-label="上级部门"]')).toHaveTextContent('研究部')
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await page.screenshot({
      path: '../../../../../.cache/organization-template/departments-light.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/organization-template/departments-dark.png'
    })
    await page.getByRole('button', { name: '研究部 · 1 级部门', exact: true }).click()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await expect
      .element(page.getByRole('textbox', { name: '部门名称', exact: true }))
      .toHaveValue('研究部')
    await page.getByRole('button', { name: '删除部门框', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '分析组 · 1 级部门', exact: true }))
      .toBeVisible()
    expect(document.querySelectorAll('[data-workflow-node-id]')).toHaveLength(2)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '分析组 · 2 级部门', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await page.getByRole('button', { name: '保存组织模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    expect(records[0].definition.nodes[0].departmentId).toBeNull()
    expect(records[0].definition.nodes[1].departmentId).toBe(child.id)
    expect(records[0].definition.departments).toHaveLength(1)
  })

  it('rejects partial overlap, cancels drawing, and undoes a frame without losing its members', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes = definition.nodes.filter((node) => node.kind === 'agent').slice(0, 1)
    Object.assign(definition.nodes[0], { x: 100, y: 100 })
    definition.viewport = { x: 0, y: 0, zoom: 1 }
    records = [{ definition, revision: 1, updatedAt: 1, enabled: true, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: `编辑组织模板 ${definition.name}`, exact: true }).click()
    await openStructure()
    await drawDepartment(40, 40, 380, 240)
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(1)
    await drawDepartment(350, 150, 220, 220)
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(1)
    // Invalid drawings remain available for another attempt; cancel explicitly.
    const cancel = document.querySelector<HTMLButtonElement>('[aria-label="取消绘制"]')
    if (cancel) await page.getByRole('button', { name: '取消绘制', exact: true }).click()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(0)
    expect(document.querySelectorAll('[data-workflow-node-id]')).toHaveLength(1)
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect
      .poll(() => document.querySelectorAll('[data-workflow-department-id]').length)
      .toBe(1)
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    await page.getByRole('button', { name: '添加部门', exact: true }).click()
    await userEvent.keyboard('{Escape}')
    await expect.poll(() => document.querySelector('[aria-label="取消绘制"]')).toBeNull()
    expect(document.querySelectorAll('[data-workflow-department-id]')).toHaveLength(1)
  })
})
