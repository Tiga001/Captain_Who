import { reviewLoopGraph } from './workflowRoutingFixtures'
import { page, userEvent } from 'vitest/browser'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'
import type {
  WorkflowRecord,
  WorkflowRequest,
  WorkflowFlow,
  WorkflowIssue
} from '@mycopilot/protocol'
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
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
const { WorkflowSettingsSection } = await import('../../features/workflows/WorkflowSettingsSection')

function WorkflowTestShell() {
  const [editing, setEditing] = useState(false)
  return (
    <div className="settings-page" data-workflow-editor={editing || undefined}>
      <div className="settings-page__drag-region" />
      <aside className="settings-nav">
        <span>设置</span>
        <span>工作流</span>
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
  await page.getByRole('tab', { name: '流程设计', exact: true }).click()
}
async function connectCards(source: string, target: string) {
  await page.getByRole('button', { name: '连接节点', exact: true }).click()
  const card = (name: string) =>
    page.getByRole('group', {
      name: name === '用户输入' ? name : `节点 ${name}`,
      exact: true
    })
  await card(source).click()
  await card(target).click()
  await page.getByRole('button', { name: '选择和拖动', exact: true }).click()
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

describe('native workflow editor', () => {
  it('edits gate names in the toolbar and shares deletion controls across all selections', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('门节点命名')
    await openStructure()
    for (const type of ['输入逻辑门', '输入逻辑门', '输出逻辑门']) {
      await page.getByRole('button', { name: '添加节点', exact: true }).click()
      await page.getByRole('button', { name: new RegExp(`^${type}`) }).click()
    }
    await page.getByRole('group', { name: '节点 输入门1', exact: true }).click()
    const toolbar = page.getByRole('group', { name: '逻辑门配置', exact: true })
    await expect
      .element(toolbar.getByRole('textbox', { name: '类型', exact: true }))
      .toHaveValue('输入逻辑门')
    await expect
      .element(toolbar.getByRole('textbox', { name: '类型', exact: true }))
      .toHaveAttribute('readonly')
    await toolbar.getByRole('textbox', { name: '节点名称', exact: true }).fill('资料汇总')
    await toolbar.getByRole('button', { name: '配置', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '输入处理方式', exact: true }))
      .toBeVisible()
    expect(document.querySelector('.workflow-graph-inspector__footer')).toBeNull()
    const deleteClass = toolbar
      .getByRole('button', { name: '删除节点', exact: true })
      .element().className
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/gate-name-toolbar.png'
    })
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    expect(records[0].definition.nodes.map((node) => node.name)).toEqual([
      '资料汇总',
      '输入门2',
      '输出门1'
    ])
    await toolbar.getByRole('button', { name: '删除节点', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 资料汇总', exact: true }))
      .not.toBeInTheDocument()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 资料汇总', exact: true }))
      .toBeVisible()
    await connectCards('用户输入', '输入门2')
    expect(page.getByRole('button', { name: '删除连线', exact: true }).element().className).toBe(
      deleteClass
    )
    await page.getByRole('button', { name: '删除连线', exact: true }).click()
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(0)
    await addBlank(false)
    expect(page.getByRole('button', { name: '删除节点', exact: true }).element().className).toBe(
      deleteClass
    )
    await page.getByRole('button', { name: '删除节点', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 未命名节点', exact: true }))
      .not.toBeInTheDocument()
  })

  it('edits selected agents in the toolbar and persists non-recycled flow names', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.flows.forEach((flow, i) => {
      flow.name = `S${i + 1}`
    })
    definition.nextFlowSequence = 24
    definition.flows[0].name = 'S2'
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    let view = await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await page.getByRole('group', { name: '节点 implement', exact: true }).click()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    const controls = page.getByRole('group', { name: '智能体快捷配置', exact: true })
    await controls.getByRole('textbox', { name: '节点名称', exact: true }).fill('交付负责人')
    await controls.getByRole('button', { name: '选择模型', exact: true }).click()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    await controls.getByRole('button', { name: '权限', exact: true }).click()
    await page.getByRole('option', { name: '自定义权限', exact: true }).click()
    await expect
      .element(controls.getByRole('button', { name: '选择模型', exact: true }))
      .toHaveTextContent('模型 B')
    await controls.getByRole('button', { name: '权限', exact: true }).click()
    await page.getByRole('option', { name: '默认权限', exact: true }).click()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await controls.getByRole('button', { name: '配置', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
      .toBeVisible()
    expect(document.querySelector('.workflow-graph-inspector__footer')).toBeNull()
    expect(document.querySelectorAll('.workflow-selection-delete')).toHaveLength(1)
    await page.screenshot({ path: '../../../../../.cache/workflow-authoring/agent-toolbar.png' })
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await page.getByRole('button', { name: '连线 1', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '连线名称', exact: true }))
      .toHaveValue('S2')
    await page.getByRole('button', { name: '删除连线', exact: true }).click()
    await connectCards('用户输入', 'Implementation input')
    await expect
      .element(page.getByRole('textbox', { name: '连线名称', exact: true }))
      .toHaveValue('S24')
    await page.getByRole('textbox', { name: '连线名称', exact: true }).fill('需求输入')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.nextFlowSequence).toBe(25)
    expect(records[0].definition.nodes[0]).toMatchObject({
      name: '交付负责人',
      modelConfigId: 'model-b',
      permissionMode: 'default'
    })
    expect(records[0].definition.flows.at(-1)?.name).toBe('需求输入')
    await view.unmount()
    view = await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await connectCards('用户输入', 'Implementation input')
    await expect
      .element(page.getByRole('textbox', { name: '连线名称', exact: true }))
      .toHaveValue('S25')
    await page.screenshot({ path: '../../../../../.cache/workflow-authoring/flow-toolbar.png' })
  })

  it('chooses anchors automatically regardless of click position, keeps manual handles and persists undo', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes = definition.nodes.filter((n) => n.kind === 'agent')
    definition.flows = []
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    let view = await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await page.getByRole('button', { name: '100%', exact: true }).click()
    const a = page.getByRole('group', { name: '节点 implement', exact: true })
    const b = page.getByRole('group', { name: '节点 review', exact: true })
    await page.getByRole('button', { name: '连接节点', exact: true }).click()
    await userEvent.dblClick(a)
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(0)
    expect(document.querySelector('.workflow-edge__preview')).not.toBeNull()
    await page.getByRole('button', { name: '选择和拖动', exact: true }).click()
    expect(document.querySelector('.workflow-edge__preview')).toBeNull()
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(0)
    await page.getByRole('button', { name: '连接节点', exact: true }).click()
    await a.click({ position: { x: 50, y: 2 } })
    await a.click({ position: { x: 100, y: 48 } })
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(0)
    expect(document.querySelector('.workflow-edge__preview')).not.toBeNull()
    await b.click({ position: { x: 80, y: 50 } })
    await page.getByRole('button', { name: '选择和拖动', exact: true }).click()
    await page.getByRole('button', { name: '终点', exact: true }).click()
    await expect.element(page.getByRole('option', { name: /^implement/ })).toBeDisabled()
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: '起点', exact: true }).click()
    await expect.element(page.getByRole('option', { name: /^review/ })).toBeDisabled()
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.flows[0].sourceAnchor).toBeUndefined()
    expect(records[0].definition.flows[0].targetAnchor).toBeUndefined()
    const before = structuredClone(records[0].definition.flows[0])
    const handle = page.getByRole('button', { name: '连线 1 起点', exact: true })
    await handle.dropTo(a, { targetPosition: { x: 2, y: 34 } })
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    const adjusted = structuredClone(records[0].definition.flows[0])
    expect(adjusted.sourceAnchor?.side).toBe('left')
    expect(adjusted.targetAnchor).toEqual(before.targetAnchor)
    expect(adjusted.source).toEqual(before.source)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(4)
    expect(records[0].definition.flows[0]).toEqual(before)
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(5)
    const position = {
      left: (handle.element() as HTMLElement).style.left,
      top: (handle.element() as HTMLElement).style.top
    }
    await view.unmount()
    view = await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    expect(records[0].definition.flows[0]).toEqual(adjusted)
    expect((handle.element() as HTMLElement).style.left).toBe(position.left)
    expect((handle.element() as HTMLElement).style.top).toBe(position.top)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/free-connection-anchors.png'
    })
  })

  it.each(['light', 'dark'] as const)(
    'optimizes the whole cyclic layout in one undoable action in %s mode',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      const definition = reviewLoopGraph()
      definition.flows.forEach((flow, i) => {
        flow.name = `S${i + 1}`
      })
      const original = structuredClone(definition)
      records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
      let view = await renderWorkflow()
      await page.getByRole('button', { name: '编辑工作流模板 分支协作与返工', exact: true }).click()
      await openStructure()
      const optimize = page.getByRole('button', { name: '优化布局', exact: true })
      const zoomControls = page.getByRole('group', { name: '适应画布', exact: true })
      expect(optimize.element().getAttribute('title')).toBe('优化布局')
      expect(optimize.element().getBoundingClientRect().right).toBeLessThan(
        zoomControls.element().getBoundingClientRect().left
      )
      await optimize.click()
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(2)
      const arranged = structuredClone(records[0].definition)
      expect(arranged.nodes).not.toEqual(original.nodes)
      expect(arranged.flows.every((flow) => !flow.sourceAnchor && !flow.targetAnchor)).toBe(true)
      const a = document.querySelector<HTMLElement>('[data-workflow-node-id="a"]')!
      const b = document.querySelector<HTMLElement>('[data-workflow-node-id="b"]')!
      const c = document.querySelector<HTMLElement>('[data-workflow-node-id="c"]')!
      expect(a.style.left).toBe(b.style.left)
      expect(b.style.left).toBe(c.style.left)
      const canvas = document.querySelector('.workflow-canvas')!.getBoundingClientRect()
      for (const item of document.querySelectorAll('.workflow-node')) {
        const rect = item.getBoundingClientRect()
        expect(rect.left).toBeGreaterThanOrEqual(canvas.left)
        expect(rect.right).toBeLessThanOrEqual(canvas.right)
        expect(rect.top).toBeGreaterThanOrEqual(canvas.top)
        expect(rect.bottom).toBeLessThanOrEqual(canvas.bottom)
      }
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/optimized-layout-${theme}.png`
      })
      await page.getByRole('button', { name: '撤销', exact: true }).click()
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(3)
      expect(records[0].definition.nodes).toEqual(original.nodes)
      expect(records[0].definition.flows).toEqual(original.flows)
      expect(records[0].definition.boundaryPositions).toEqual(original.boundaryPositions)
      await page.getByRole('button', { name: '重做', exact: true }).click()
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(4)
      await view.unmount()
      view = await renderWorkflow()
      await page.getByRole('button', { name: '编辑工作流模板 分支协作与返工', exact: true }).click()
      await openStructure()
      expect(records[0].definition.nodes).toEqual(arranged.nodes)
      expect(records[0].definition.flows).toEqual(arranged.flows)
      await optimize.click()
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(5)
      expect(records[0].definition.nodes).toEqual(arranged.nodes)
      expect(records[0].definition.flows).toEqual(arranged.flows)
    }
  )

  it.each(['light', 'dark'] as const)(
    'uses compact app menus for adding nodes and connections in %s mode',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      const definition = parseWorkflowDefinition(fixture)
      definition.nodes.forEach((node) => {
        if (node.kind === 'agent') node.modelConfigId = 'model'
      })
      records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
      await renderWorkflow()
      await page
        .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
        .click()
      await openStructure()
      await page.getByRole('button', { name: '添加节点', exact: true }).click()
      await expect
        .element(page.getByRole('textbox', { name: '搜索节点', exact: true }))
        .toHaveFocus()
      await expect
        .element(page.getByRole('button', { name: '新建智能体', exact: true }))
        .toBeVisible()
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/node-palette-${theme}.png`
      })
      await page.getByRole('button', { name: '连接节点', exact: true }).click()
      expect(document.querySelector('.workflow-connect-picker')).toBeNull()
      await expect
        .element(page.getByRole('button', { name: '连接节点', exact: true }))
        .toHaveAttribute('aria-pressed', 'true')
      await page.getByRole('group', { name: '节点 review', exact: true }).click()
      expect(document.querySelector('.workflow-edge__preview')).not.toBeNull()
      await page.getByRole('group', { name: '节点 验收', exact: true }).click()
      expect(document.querySelector('.workflow-edge__preview')).toBeNull()
      await expect
        .element(page.getByRole('button', { name: '连接节点', exact: true }))
        .toHaveAttribute('aria-pressed', 'true')
      await page.getByRole('button', { name: '选择和拖动', exact: true }).click()
      expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
      expect(document.querySelector('.workflow-edge__caption')).toBeNull()
      expect(document.querySelector('.workflow-page-header .workflow-page-tabs')).not.toBeNull()
      const edgeControls = document.querySelector('.workflow-edge-controls')!
      expect(edgeControls.querySelectorAll('button')).toHaveLength(3)
      expect(edgeControls.querySelector('input')?.value).toMatch(/^S\d+$/)
      const toolbarRect = document.querySelector('.workflow-graph-toolbar')!.getBoundingClientRect()
      const edgeRect = edgeControls.getBoundingClientRect()
      expect(edgeRect.left).toBeGreaterThan(toolbarRect.right)
      expect(Math.abs(edgeRect.top - toolbarRect.top)).toBeLessThan(4)
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/compact-toolbar-${theme}.png`
      })
      const inspectorSource = page.getByRole('button', { name: '起点', exact: true })
      await inspectorSource.click()
      await expect
        .element(page.getByRole('option', { name: '用户输入', exact: true }))
        .not.toBeDisabled()
      await userEvent.keyboard('{Escape}')
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(2)
      expect(records[0].definition.flows.at(-1)).toMatchObject({
        source: { kind: 'node' },
        target: { kind: 'node', nodeId: 'result' }
      })
    }
  )

  it.each(['light', 'dark'] as const)(
    'edits independent triangular gates and persists thresholds, arrival policy and output selection in %s',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      const view = await renderWorkflow()
      await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
      await page.getByRole('textbox', { name: '名称', exact: true }).fill('多方评审与交付')
      await openStructure()
      for (const name of ['输入逻辑门', '输出逻辑门']) {
        await page.getByRole('button', { name: '添加节点', exact: true }).click()
        await page.getByRole('button', { name: new RegExp('^' + name) }).click()
        expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
      }
      const inputCard = page.getByRole('group', { name: '节点 输入门1', exact: true })
      const outputCard = page.getByRole('group', { name: '节点 输出门1', exact: true })
      expect(inputCard.element().querySelector('svg path')?.getAttribute('d')).not.toBe(
        outputCard.element().querySelector('svg path')?.getAttribute('d')
      )
      await inputCard.click({ position: { x: 12, y: 10 } })
      const oldLeft = (inputCard.element() as HTMLElement).style.left
      await userEvent.keyboard('{ArrowRight}')
      expect((inputCard.element() as HTMLElement).style.left).not.toBe(oldLeft)
      expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
      await addBlank()
      await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('综合评审')
      await page
        .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
        .fill('综合各方结论并分发结果。')
      await connectCards('输入门1', '综合评审')
      await connectCards('综合评审', '输出门1')
      for (let i = 0; i < 3; i++) {
        await connectCards('用户输入', '输入门1')
        await addBlank(false)
        await page.getByRole('textbox', { name: '节点名称', exact: true }).fill(`交付 ${i}`)
        await connectCards('输出门1', `交付 ${i}`)
      }
      await configureNode('输入门1')
      await expect
        .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
        .toHaveValue('输入门1')
      expect((inputCard.element() as HTMLElement).style.width).toBe('64px')
      expect(inputCard.element().querySelector('svg text')).toBeNull()
      expect(document.querySelector('.workflow-node__configure')).toBeNull()
      const primaryColor = getComputedStyle(
        document.querySelector('.workflow-canvas-controls button')!
      ).color
      for (const selector of [
        '.workflow-edge',
        '.workflow-canvas-controls button',
        '.workflow-graph-toolbar__add svg'
      ]) {
        expect(getComputedStyle(document.querySelector(selector)!).color).toBe(primaryColor)
      }
      expect(
        getComputedStyle(inputCard.element().querySelector('.workflow-gate-shape path')!).stroke
      ).not.toBe(primaryColor)
      expect(getComputedStyle(document.querySelector('.workflow-edge__line')!).opacity).toBe('1')
      const agentCard = page.getByRole('group', { name: '节点 综合评审', exact: true }).element()
      expect(getComputedStyle(agentCard).backgroundColor).toBe(primaryColor)
      const cardTextColor = getComputedStyle(agentCard.querySelector('strong')!).color
      expect(
        getComputedStyle(inputCard.element().querySelector('.workflow-gate-shape path')!).fill
      ).toBe(primaryColor)
      expect(cardTextColor).not.toBe(primaryColor)
      expect(agentCard.querySelector('.agent-avatar img')).not.toBeNull()
      expect((agentCard as HTMLElement).style.height).toBe('52px')

      await expect
        .element(page.getByRole('button', { name: '子智能体模板', exact: true }))
        .not.toBeInTheDocument()
      await expect
        .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
        .not.toBeInTheDocument()
      await page.getByRole('button', { name: '输入处理方式', exact: true }).click()
      expect(document.querySelectorAll('[role="option"]')).toHaveLength(2)
      await page.getByRole('option', { name: /^逐条处理/ }).click()
      expect(document.querySelector('.workflow-graph-inspector input[type="number"]')).toBeNull()
      expect(document.querySelector('.workflow-rule__members')).toBeNull()
      await page.getByRole('button', { name: '智能体忙碌时', exact: true }).click()
      await page.getByRole('option', { name: /^插入消息/ }).click()
      expect(inputCard.element().textContent).toBe('')
      await page.getByRole('button', { name: '适应画布', exact: true }).click()
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/logic-gates-${theme}.png`
      })
      await page.getByRole('group', { name: '节点 综合评审', exact: true }).click()
      await expect
        .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
        .toHaveValue('综合各方结论并分发结果。')
      await inputCard.click()
      await expect
        .element(page.getByRole('button', { name: '输入处理方式', exact: true }))
        .toHaveTextContent('逐条处理')
      await expect
        .element(page.getByRole('button', { name: '智能体忙碌时', exact: true }))
        .toHaveTextContent('插入消息')
      await outputCard.click()
      expect(
        getComputedStyle(outputCard.element().querySelector('.workflow-gate-shape path')!).stroke
      ).not.toBe(primaryColor)
      expect(
        getComputedStyle(inputCard.element().querySelector('.workflow-gate-shape path')!).stroke
      ).toBe(primaryColor)
      await page.getByRole('button', { name: '出口选择规则', exact: true }).click()
      await page.getByRole('option', { name: '选择 N 条', exact: true }).click()
      await page.getByRole('spinbutton', { name: '出口选择规则 选择数量', exact: true }).fill('2')
      expect(outputCard.element().textContent).toBe('')
      await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
      await page
        .getByRole('button', { name: '连线 7 终点', exact: true })
        .dropTo(inputCard, { targetPosition: { x: 2, y: 8 } })
      await page
        .getByRole('button', { name: '连线 1 起点', exact: true })
        .dropTo(inputCard, { targetPosition: { x: 2, y: 8 } })
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records.length).toBe(1)
      expect(records[0].definition.nodes).toHaveLength(6)
      expect(records[0].definition.flows).toHaveLength(8)
      expect(records[0].definition.flows[0].sourceAnchor).toEqual({ side: 'right', offset: 0.5 })
      expect(records[0].definition.flows[6].targetAnchor?.side).toBe('left')
      expect(records[0].definition.flows[6].targetAnchor!.offset).toBeLessThan(0.5)
      expect(records[0].definition.nodes.find((n) => n.kind === 'inputGate')).toMatchObject({
        processingMode: 'individual',
        busyPolicy: 'inject'
      })
      expect(records[0].definition.nodes.find((n) => n.kind === 'outputGate')).toMatchObject({
        selection: { mode: 'exact', min: 2 }
      })
      await view.unmount()
      await renderWorkflow()
      await page.getByRole('button', { name: '编辑工作流模板 多方评审与交付', exact: true }).click()
      await openStructure()
      await configureNode('输入门1')
      await expect
        .element(page.getByRole('button', { name: '输入处理方式', exact: true }))
        .toHaveTextContent('逐条处理')
      await expect
        .element(page.getByRole('button', { name: '智能体忙碌时', exact: true }))
        .toHaveTextContent('插入消息')
      await page.getByRole('button', { name: '智能体忙碌时', exact: true }).click()
      await page.getByRole('option', { name: /^排队等候/ }).click()
      await page.getByRole('button', { name: '撤销', exact: true }).click()
      await expect
        .element(page.getByRole('button', { name: '智能体忙碌时', exact: true }))
        .toHaveTextContent('插入消息')
    }
  )

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
        issues: [{ code: 'entry', subject: '' }]
      }
    ]
    const view = await renderWorkflow()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 版本发布验收', exact: true }))
      .toBeVisible()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 未完成流程', exact: true }))
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
      .element(page.getByRole('button', { name: '编辑工作流模板 版本发布验收', exact: true }))
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
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    validationIssues = [{ code: 'name', subject: '' }]
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].enabled).toBe(false)
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    expect(page.getByRole('switch').query()).toBeNull()
    await expect.element(page.getByText('草稿', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '编辑工作流模板', exact: true }).click()
    validationIssues = []
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('版本发布验收')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    expect(page.getByRole('switch').query()).toBeNull()
    expect(records[0].enabled).toBe(true)
    expect(page.getByText('草稿', { exact: true }).query()).toBeNull()
    expect(document.body.textContent).not.toContain('图结构已校验')
  })

  it('configures a user task without model fields and persists the account identity and task', async () => {
    const avatar =
      'data:image/svg+xml,' +
      encodeURIComponent(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" fill="#58a"/></svg>'
      )
    service.profile = {
      userId: 'local-user',
      displayName: '测试用户',
      avatarDataUrl: avatar,
      email: '',
      occupation: '',
      organization: ''
    }
    let view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('用户参与流程')
    await openStructure()
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    const list = document.querySelector('.workflow-node-picker__list')!
    expect(list.querySelector('button')!.textContent).toBe('新建智能体')
    expect(list.previousElementSibling!.textContent).toBe('智能体')
    const canvas = page.elementLocator(document.querySelector('.workflow-canvas')!)
    await page
      .getByRole('button', { name: '用户', exact: true })
      .dropTo(canvas, { targetPosition: { x: 440, y: 240 } })
    const user = page.getByRole('group', { name: '节点 测试用户', exact: true })
    await expect.element(user).toBeVisible()
    expect(user.element().querySelector('img')!.getAttribute('src')).toBe(avatar)
    expect(user.element().classList.contains('workflow-node--gate')).toBe(false)
    await userEvent.dblClick(user)
    const task = page.getByRole('textbox', { name: '需要用户做什么', exact: true })
    await task.fill('检查方案，确认后通知开发智能体继续。')
    expect(document.querySelectorAll('.workflow-graph-inspector textarea')).toHaveLength(1)
    await expect
      .element(page.getByRole('button', { name: '子智能体模板', exact: true }))
      .not.toBeInTheDocument()
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    const before = (user.element() as HTMLElement).style.left
    await userEvent.keyboard('{ArrowRight}')
    expect((user.element() as HTMLElement).style.left).not.toBe(before)
    await page.getByRole('button', { name: '优化布局', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    expect(records[0].definition.nodes).toHaveLength(1)
    expect(records[0].definition.nodes[0]).toMatchObject({
      kind: 'user',
      name: '测试用户',
      task: '检查方案，确认后通知开发智能体继续。'
    })
    expect(records[0].definition.nodes[0]).not.toHaveProperty('modelConfigId')
    await page.screenshot({ path: '../../../../../.cache/workflow-authoring/user-node.png' })
    await view.unmount()
    view = await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 用户参与流程', exact: true }).click()
    await openStructure()
    await expect.element(user).toBeVisible()
    expect(user.element().querySelector('img')!.getAttribute('src')).toBe(avatar)
    await user.click()
    await page.getByRole('button', { name: '配置', exact: true }).click()
    await expect.element(task).toHaveValue('检查方案，确认后通知开发智能体继续。')
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/user-node-configuration.png'
    })
    await page.getByRole('button', { name: '删除节点', exact: true }).click()
    await expect.element(user).not.toBeInTheDocument()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect.element(user).toBeVisible()
  })

  it('automatically gates user inputs while keeping user outputs direct', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes = definition.nodes.filter((node) => node.kind === 'agent')
    definition.flows = []
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    await page.getByRole('button', { name: '用户', exact: true }).click()
    await configureNode('当前用户')
    await page
      .getByRole('textbox', { name: '需要用户做什么', exact: true })
      .fill('检查两方意见，分别回复。')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await connectCards('implement', '当前用户')
    await connectCards('review', '当前用户')
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await configureNode('输入门1')
    await expect
      .element(page.getByRole('button', { name: '输入处理方式', exact: true }))
      .toHaveTextContent('按批次处理')
    await page.getByRole('button', { name: '用户处理期间', exact: true }).click()
    await page.getByRole('option', { name: /^插入消息/ }).click()
    await page.getByRole('group', { name: '节点 当前用户', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '需要用户做什么', exact: true }))
      .toHaveValue('检查两方意见，分别回复。')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await connectCards('当前用户', 'implement')
    await connectCards('当前用户', 'review')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    const saved = records[0].definition
    const user = saved.nodes.find((node) => node.kind === 'user')!
    const input = saved.nodes.find((node) => node.kind === 'inputGate')!
    expect(saved.nodes.filter((node) => node.kind === 'inputGate')).toHaveLength(1)
    expect(saved.nodes.some((node) => node.kind === 'outputGate')).toBe(false)
    expect(
      saved.flows.filter((flow) => flow.source.kind === 'node' && flow.source.nodeId === user.id)
    ).toHaveLength(2)
    expect(
      saved.flows.filter((flow) => flow.target.kind === 'node' && flow.target.nodeId === user.id)
    ).toHaveLength(1)
    expect(input).toMatchObject({ processingMode: 'batch', busyPolicy: 'inject' })
    await page.getByRole('button', { name: '优化布局', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    const arrangedUser = records[0].definition.nodes.find((node) => node.id === user.id)!
    const arrangedGate = records[0].definition.nodes.find((node) => node.id === input.id)!
    expect(arrangedGate.x).toBeLessThan(arrangedUser.x)
    expect(arrangedGate.y + 28).toBe(arrangedUser.y + 26)
  })

  it.each(['light', 'dark'] as const)(
    'keeps icon tools and agent settings on one row with hover help in %s',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      await renderWorkflow()
      await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
      await openStructure()
      await addBlank(false)
      for (const width of [1152, 900]) {
        await page.viewport(width, 760)
        const labels = ['添加节点', '连接节点', '选择和拖动', '配置', '删除节点']
        const buttons = labels.map((name) =>
          page.getByRole('button', { name, exact: true }).element()
        )
        const top = buttons[0].getBoundingClientRect().top
        for (const button of buttons) {
          expect(Math.abs(button.getBoundingClientRect().top - top)).toBeLessThan(2)
          expect(button.textContent).toBe('')
          const icon = button.querySelector('svg')!
          expect(getComputedStyle(icon).width).toBe('16px')
          expect(getComputedStyle(icon).height).toBe('16px')
          expect(button.getBoundingClientRect().right).toBeLessThanOrEqual(width)
        }
      }
      for (const name of ['添加节点', '连接节点', '配置']) {
        await page.getByRole('button', { name, exact: true }).hover()
        await expect.element(page.getByRole('tooltip')).toHaveTextContent(name)
      }
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/single-row-user-entry-${theme}.png`
      })
    }
  )

  it('creates one movable user entry and saves a terminal agent without a fixed output', async () => {
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('版本发布验收')
    await openStructure()
    const inputRoot = page.getByRole('group', { name: '用户输入', exact: true })
    expect(document.querySelectorAll('.workflow-node--root')).toHaveLength(1)
    expect(inputRoot.element().querySelector('.workflow-user-avatar img')).not.toBeNull()
    await expect.element(inputRoot).toBeVisible()
    const canvas = page.elementLocator(document.querySelector('.workflow-canvas')!)
    await inputRoot.dropTo(canvas, { targetPosition: { x: 160, y: 170 } })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    const movedRoots = structuredClone(records[0].definition.boundaryPositions)
    expect(movedRoots.input).not.toEqual({ x: 80, y: 220 })
    expect(records[0].definition.nodes).toEqual([])
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.boundaryPositions).toEqual({
      input: { x: 80, y: 220 }
    })
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await addBlank(false)
    const node = page.getByRole('group', { name: '节点 未命名节点', exact: true })
    await node.click()
    await userEvent.keyboard('{ArrowRight}')
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await node.dropTo(canvas, { targetPosition: { x: 480, y: 350 } })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    const avatar = node.element().querySelector('.agent-avatar img')!.getAttribute('src')
    await configureNode('未命名节点')
    expect(document.querySelector('.workflow-shared-background')).toBeNull()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('验收')
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('验证交付物并报告验收结果。')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    const renamedNode = page.getByRole('group', { name: '节点 验收', exact: true })
    await expect.element(renamedNode).toHaveFocus()
    await userEvent.keyboard('{Enter}')
    await expect
      .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
      .toHaveValue('验收')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await connectCards('用户输入', '验收')
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await connectCards('验收', '用户输入')
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(1)
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await userEvent.keyboard('{Escape}')
    await inputRoot.click()
    await userEvent.keyboard('{Delete}')
    await expect.element(inputRoot).toBeVisible()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    const saved = records[0].definition
    expect(saved.boundaryPositions).toEqual(movedRoots)
    expect(saved.nodes).toHaveLength(1)
    expect(saved.flows).toHaveLength(1)
    expect(saved.flows[0].source).toEqual({ kind: 'boundary' })
    const rootLeft = (inputRoot.element() as HTMLElement).style.left
    await view.unmount()
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 版本发布验收', exact: true }).click()
    await openStructure()
    await expect.element(inputRoot).toBeVisible()
    await expect.poll(() => (inputRoot.element() as HTMLElement).style.left).toBe(rootLeft)
    expect(
      page
        .getByRole('group', { name: '节点 验收', exact: true })
        .element()
        .querySelector('.agent-avatar img')!
        .getAttribute('src')
    ).toBe(avatar)
    await expect
      .poll(
        () =>
          inputRoot.element().getBoundingClientRect().left -
          document.querySelector('.workflow-canvas')!.getBoundingClientRect().left
      )
      .toBeGreaterThanOrEqual(0)
    await page.screenshot({ path: '../../../../../.cache/workflow-authoring/root-nodes.png' })
  })

  it('keeps parallel development routes within their local corridors beneath the roots', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.name = '并行开发与评审'
    const base = definition.nodes[0]
    if (base.kind !== 'agent') throw new Error('Expected agent')
    definition.nodes = [
      { id: 'prepare', name: '任务拆解', x: 80, y: 330 },
      { id: 'frontend', name: '前端开发', x: 440, y: 160 },
      { id: 'backend', name: '后端开发', x: 390, y: 330 },
      { id: 'tests', name: '测试开发', x: 440, y: 500 },
      { id: 'review', name: '结果评审', x: 760, y: 330 }
    ].map((node) => ({
      ...structuredClone(base),
      ...node,
      modelConfigId: 'model'
    }))
    definition.boundaryPositions = {
      input: { x: 760, y: 20 }
    }
    const developmentIds = ['frontend', 'backend', 'tests']
    const edge = (id: string, source: string, target: string): WorkflowFlow => ({
      id,
      name: '',
      source: { kind: 'node', nodeId: source },
      target: { kind: 'node', nodeId: target }
    })
    const forwardFlows = [
      ...developmentIds.map((id) => edge(`assign-${id}`, 'prepare', id)),
      ...developmentIds.map((id) => edge(`review-${id}`, id, 'review'))
    ]
    definition.flows = [
      {
        id: 'entry',
        name: '',
        source: { kind: 'boundary' },
        target: { kind: 'node', nodeId: 'prepare' }
      },
      ...forwardFlows
    ]
    definition.viewport = { x: 0, y: 0, zoom: 1 }
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 并行开发与评审', exact: true }).click()
    await openStructure()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    for (const node of definition.nodes)
      await expect
        .element(page.getByRole('group', { name: `节点 ${node.name}`, exact: true }))
        .toBeVisible()
    for (const root of ['用户输入'])
      await expect.element(page.getByRole('group', { name: root, exact: true })).toBeVisible()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/parallel-development-routing.png'
    })
    // These staggered cards leave clear corridors. Ordinary forward branches must
    // stay beside their endpoints, rather than detouring above the unrelated roots.
    for (const flow of forwardFlows) {
      const index = definition.flows.indexOf(flow)
      const path = page
        .getByRole('button', { name: `连线 ${index + 1}`, exact: true })
        .element()
        .querySelector<SVGPathElement>('.workflow-edge__line')!
      const source = definition.nodes.find(
        (node) => flow.source.kind === 'node' && node.id === flow.source.nodeId
      )!
      const target = definition.nodes.find(
        (node) => flow.target.kind === 'node' && node.id === flow.target.nodeId
      )!
      const bounds = path.getBBox()
      expect(bounds.y).toBeGreaterThanOrEqual(Math.min(source.y, target.y) - 16)
      expect(bounds.y + bounds.height).toBeLessThanOrEqual(Math.max(source.y, target.y) + 80)
    }
  })

  it.each(['light', 'dark'] as const)(
    'retains bridges for crossings that need excessive detours in the %s theme',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      const definition = parseWorkflowDefinition(fixture)
      definition.name = '独立任务交叉连线'
      const base = definition.nodes[0]
      if (base.kind !== 'agent') throw new Error('Expected agent')
      definition.nodes = [
        { id: 'horizontal-source', name: '横向任务', x: 80, y: 300 },
        { id: 'horizontal-target', name: '横向交付', x: 820, y: 300 },
        { id: 'vertical-source', name: '纵向任务', x: 200, y: 100 },
        { id: 'vertical-target', name: '纵向交付', x: 700, y: 500 }
      ].map((node) => ({ ...structuredClone(base), ...node, modelConfigId: 'model' }))
      definition.boundaryPositions = {
        input: { x: 80, y: 20 }
      }
      definition.flows = [
        {
          id: 'horizontal',
          name: '',
          source: { kind: 'node', nodeId: 'horizontal-source' },
          target: { kind: 'node', nodeId: 'horizontal-target' }
        },
        {
          id: 'vertical',
          name: '',
          source: { kind: 'node', nodeId: 'vertical-source' },
          target: { kind: 'node', nodeId: 'vertical-target' }
        }
      ]
      definition.viewport = { x: 0, y: 0, zoom: 1 }
      definition.flows.forEach((flow, i) => {
        flow.name = `S${i + 1}`
      })
      const originalFlows = structuredClone(definition.flows)
      records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
      await renderWorkflow()
      await page
        .getByRole('button', { name: '编辑工作流模板 独立任务交叉连线', exact: true })
        .click()
      await openStructure()
      await page.getByRole('button', { name: '适应画布', exact: true }).click()
      await page.getByRole('button', { name: '100%', exact: true }).click()
      await expect
        .element(page.getByRole('group', { name: '节点 横向任务', exact: true }))
        .toBeVisible()
      await expect
        .element(page.getByRole('group', { name: '节点 纵向交付', exact: true }))
        .toBeVisible()
      const bridges = document.querySelectorAll('.workflow-crossing')
      expect(bridges).toHaveLength(1)
      const bridge = bridges[0].querySelector<SVGPathElement>('.workflow-crossing__line')!
      const halo = bridges[0].querySelector<SVGPathElement>('.workflow-crossing__halo')!
      expect(bridge.getAttribute('d')).toContain(' A ')
      expect(getComputedStyle(halo).stroke).toBe(
        getComputedStyle(document.querySelector('.workflow-canvas-shell')!).backgroundColor
      )
      const bridgeWidth = bridge.getBoundingClientRect().width
      const edge = page.getByRole('button', { name: '连线 2', exact: true })
      const line = edge.element().querySelector<SVGPathElement>('.workflow-edge__line')!
      const path = line.getAttribute('d')
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/uncrossed-${theme}.png`
      })
      ;(edge.element() as SVGElement).focus()
      await userEvent.keyboard('{Enter}')
      expect(edge.element().classList.contains('is-selected')).toBe(true)
      expect(getComputedStyle(line).strokeWidth).toBe('2px')
      for (let i = 0; i < 3; i++)
        await page.getByRole('button', { name: '放大', exact: true }).click()
      expect(line.getAttribute('d')).toBe(path)
      expect(bridge.getBoundingClientRect().width / bridgeWidth).toBeCloseTo(1.3, 2)
      expect(getComputedStyle(bridge).strokeWidth).toBe('2px')
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(2)
      expect(records[0].definition.flows).toEqual(originalFlows)
    }
  )

  it('routes parallel review branches and return loops without changing the saved graph', async () => {
    const definition = reviewLoopGraph()
    records = [{ definition, revision: 1, updatedAt: 1, enabled: false, issues: [] }]
    definition.flows.forEach((flow, i) => {
      flow.name = `S${i + 1}`
    })
    const snapshot = structuredClone(definition)
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 分支协作与返工', exact: true }).click()
    await openStructure()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    expect(document.querySelectorAll('.workflow-edge')).toHaveLength(15)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/review-loop-routing.png'
    })
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.nodes).toEqual(snapshot.nodes)
    expect(records[0].definition.flows).toEqual(snapshot.flows)
  })

  it('shares metadata and graph across tabs and persists the whole workflow with one save', async () => {
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveAttribute('placeholder', '例如：产品方案评审、版本发布验收')
    await expect
      .element(page.getByRole('textbox', { name: '简短描述', exact: true }))
      .toHaveAttribute('placeholder', '说明这个工作流适合处理什么任务、何时使用')
    await expect
      .element(page.getByRole('textbox', { name: '工作流公共背景', exact: true }))
      .toHaveAttribute('placeholder', '填写所有节点都需要了解的目标、背景和共同要求')
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-details.png'
    })
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('验证码开发')
    await page.getByRole('textbox', { name: '简短描述', exact: true }).fill('分析、开发和审查')
    await page.getByRole('textbox', { name: '工作流公共背景', exact: true }).fill('实现登录功能')
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
      .element(page.getByRole('textbox', { name: '工作流公共背景', exact: true }))
      .toHaveValue('实现登录功能')
    await openStructure()
    await expect.element(page.getByRole('group', { name: '节点 开发', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
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
    await expect.element(page.getByRole('tab', { name: '流程设计', exact: true })).toBeVisible()
    await expect
      .element(page.getByRole('status', { name: '未保存', exact: true }))
      .not.toBeInTheDocument()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-node.png'
    })
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    expect(document.body.textContent).not.toMatch(
      /定义校验|工作流已保存|可复用的协作流程|当前可设计和保存工作流模板|个节点|条连线/
    )
    await view.unmount()
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 验证码开发', exact: true }).click()
    await openStructure()
    await expect.element(page.getByRole('group', { name: '节点 开发', exact: true })).toBeVisible()
    await expect
      .element(page.getByRole('group', { name: '节点 开发', exact: true }))
      .toHaveTextContent('模型 B')
  })

  it('undoes and redoes node deletion including its cycle, then protects discarded edits', async () => {
    records = [
      {
        definition: parseWorkflowDefinition(fixture),
        revision: 3,
        updatedAt: 1,
        enabled: false,
        issues: []
      }
    ]
    await renderWorkflow()
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-canvas.png'
    })
    await configureNode('review')
    await page.getByRole('button', { name: '删除节点', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 review', exact: true }))
      .not.toBeInTheDocument()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 review', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect
      .element(page.getByRole('group', { name: '节点 review', exact: true }))
      .not.toBeInTheDocument()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(4)
    expect(records[0].definition.nodes).toHaveLength(5)
    expect(records[0].definition.flows).toHaveLength(6)
    expect(records[0].definition.nodes[3]).toMatchObject({
      kind: 'outputGate',
      selection: { mode: 'one' }
    })
    await addBlank()
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    await expect
      .element(page.getByRole('dialog', { name: '放弃未保存的修改？', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '放弃修改', exact: true }).click()
    expect(records[0].definition.nodes).toHaveLength(5)
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true }))
      .toBeVisible()
  })

  it('adds a conversation agent by drag and saves entry, loop, exit and a one-of-two selection group', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('开发和审查')
    await openStructure()
    await addBlank()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('开发')
    await page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }).fill('实现需求。')
    await connectCards('用户输入', '开发')
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    await page
      .getByRole('button', { name: '新建智能体', exact: true })
      .dropTo(page.elementLocator(document.querySelector('.workflow-canvas')!), {
        targetPosition: { x: 250, y: 530 }
      })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await configureNode('未命名节点')
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('审查员模板')
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('核实实际问题并附证据。')
    await expect
      .element(page.getByRole('button', { name: '选择模型', exact: true }))
      .toHaveTextContent('模型 A')
    await connectCards('开发', '审查员模板')
    await connectCards('审查员模板', '开发')
    await addBlank(false)
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('交付验收')
    await connectCards('审查员模板', '交付验收')
    await configureNode('输入门1')
    await page.getByRole('button', { name: '输入处理方式', exact: true }).click()
    await page.getByRole('option', { name: /^逐条处理/ }).click()
    await configureNode('输出门1')
    await page.getByRole('button', { name: '出口选择规则', exact: true }).click()
    await page.getByRole('option', { name: '自定义组合', exact: true }).click()
    await page.getByRole('button', { name: '添加分组', exact: true }).click()
    const group = page.getByRole('group', { name: '选择分组 1', exact: true })
    await group.getByRole('checkbox').nth(0).click()
    await group.getByRole('checkbox').nth(1).click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-rules.png'
    })
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    const definition = records[0].definition
    expect(definition.nodes).toHaveLength(5)
    expect(definition.flows).toHaveLength(6)
    expect(definition.nodes.find((node) => node.kind === 'inputGate')).toMatchObject({
      processingMode: 'individual'
    })
    const reviewer = definition.nodes.find(
      (node) => node.kind === 'agent' && node.name === '审查员模板'
    )!
    expect(reviewer).toMatchObject({ modelConfigId: 'model', permissionMode: 'default' })
    const outputGate = definition.nodes.find((node) => node.kind === 'outputGate')!
    expect(outputGate.selection).toMatchObject({ mode: 'custom', groups: [{ min: 1, max: 1 }] })
    expect(new Set(outputGate.selection.groups[0].flowIds)).toEqual(
      new Set(
        definition.flows
          .filter((flow) => flow.source.kind === 'node' && flow.source.nodeId === outputGate.id)
          .map((flow) => flow.id)
      )
    )
    await page.viewport(900, 760)
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await expect.element(page.getByRole('button', { name: '适应画布', exact: true })).toBeVisible()
  })

  it('edits conversation permissions independently of models and preserves them after reopening', async () => {
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    let view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
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
      await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
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
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect
      .poll(() => records[0]?.definition.nodes[0])
      .toMatchObject({ permissionMode: 'custom' })
    await view.unmount()
    view = await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流模板 独立对话协作', exact: true }).click()
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
    await page
      .getByRole('button', { name: '编辑工作流模板 Develop and review', exact: true })
      .click()
    await openStructure()
    await configureNode('implement')
    const modelPicker = page.getByRole('button', { name: '选择模型', exact: true })
    await expect.element(modelPicker).toHaveTextContent('旧模型 · 模型不可用')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
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
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await openStructure()
    await addBlank()
    await expect.element(modelPicker).toBeDisabled()
    await expect.element(modelPicker).toHaveTextContent('暂无可用模型')
    await expect
      .element(page.getByRole('group', { name: '节点 未命名节点', exact: true }))
      .toHaveTextContent('未选择模型')
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    await expect.poll(() => records[0]?.definition.nodes[0]).toMatchObject({ modelConfigId: null })
  })

  it('shows save issues only in an acknowledgement dialog and leaves the saved editor ready', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流模板', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('草稿流程')
    validationIssues = [
      { code: 'empty', subject: '' },
      { code: 'entry', subject: '' },
      { code: 'entry', subject: '' }
    ]
    await page.getByRole('button', { name: '保存工作流模板', exact: true }).click()
    const dialog = page.getByRole('alertdialog', { name: '已保存为草稿', exact: true })
    await expect.element(dialog).toHaveTextContent('请添加至少一个智能体节点。')
    await dialog.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(dialog).not.toBeInTheDocument()
    await expect
      .element(page.getByRole('button', { name: '保存工作流模板', exact: true }))
      .toHaveFocus()
    expect(document.body.textContent).not.toContain('请添加至少一个智能体节点。')
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual([
      'list',
      'save'
    ])
    await page.getByRole('button', { name: '返回工作流模板列表', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流模板 草稿流程', exact: true }))
      .toBeVisible()
  })
})
