import { page, userEvent } from 'vitest/browser'
import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import '../../styles/global.css'
import '../../features/settings/SettingsPage.css'
import type {
  AgentTemplate,
  WorkflowRecord,
  WorkflowRequest,
  WorkflowFlow,
  WorkflowIssue
} from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { parseWorkflowDefinition } from '@mycopilot/protocol'

const service = vi.hoisted(() => ({
  request: vi.fn(),
  models: [] as Array<{
    id: string
    displayName: string
    execution: { status: 'available' } | { status: 'unavailable'; reason: 'disabled' }
  }>
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
    t: (key: string) => (key === 'settings.breadcrumb.root' ? '设置' : '当前位置')
  })
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
const { WorkflowSettingsSection } = await import('../../features/workflows/WorkflowSettingsSection')

const templates: AgentTemplate[] = [
  {
    schemaVersion: 1,
    templateId: 'review-template',
    machineKey: 'reviewer',
    name: '审查员模板',
    instructions: '核实实际问题并附证据。',
    description: '代码审查',
    modelConfigId: 'model',
    modelDisplayName: '模型 A',
    projectIds: [],
    enabled: true,
    revision: 1,
    createdAt: 1,
    updatedAt: 1
  }
]
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
          <WorkflowSettingsSection templates={templates} onEditorModeChange={setEditing} />
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
async function configureNode(name: string) {
  await page.getByRole('button', { name: `配置节点 ${name}`, exact: true }).click()
}
async function addBlank(configure = true) {
  await page.getByRole('button', { name: '添加节点', exact: true }).click()
  await page.getByRole('button', { name: '新建空白节点', exact: true }).click()
  expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
  if (configure) await configureNode('未命名节点')
}

let records: WorkflowRecord[]
let validationIssues: WorkflowIssue[]
beforeEach(async () => {
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
  it('creates movable roots, opens node settings only from the gear, and reconnects persisted boundaries', async () => {
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('版本发布验收')
    await openStructure()
    const inputRoot = page.getByRole('group', { name: '根智能体 · 输入', exact: true })
    const outputRoot = page.getByRole('group', { name: '根智能体 · 输出', exact: true })
    await expect.element(inputRoot).toBeVisible()
    await expect.element(outputRoot).toBeVisible()
    const canvas = page.elementLocator(document.querySelector('.workflow-canvas')!)
    await inputRoot.dropTo(canvas, { targetPosition: { x: 160, y: 170 } })
    await outputRoot.dropTo(canvas, { targetPosition: { x: 800, y: 420 } })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    const movedRoots = structuredClone(records[0].definition.boundaryPositions)
    expect(movedRoots.input).not.toEqual({ x: 80, y: 220 })
    expect(movedRoots.output).not.toEqual({ x: 760, y: 220 })
    expect(records[0].definition.nodes).toEqual([])
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.boundaryPositions).toEqual({
      input: movedRoots.input,
      output: { x: 760, y: 220 }
    })
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await addBlank(false)
    const node = page.getByRole('group', { name: '节点 未命名节点', exact: true })
    await node.click()
    await userEvent.keyboard('{ArrowRight}')
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await node.dropTo(canvas, { targetPosition: { x: 480, y: 350 } })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await configureNode('未命名节点')
    expect(document.querySelector('.workflow-shared-background')).toBeNull()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('验收')
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('验证交付物并报告验收结果。')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await page
      .getByRole('button', { name: '根智能体 · 输入 出流端口', exact: true })
      .dropTo(page.getByRole('button', { name: '验收 入流端口', exact: true }))
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await page
      .getByRole('button', { name: '验收 出流端口', exact: true })
      .dropTo(page.getByRole('button', { name: '根智能体 · 输出 入流端口', exact: true }))
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await inputRoot.click()
    await userEvent.keyboard('{Delete}')
    await expect.element(inputRoot).toBeVisible()
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(3)
    const saved = records[0].definition
    expect(saved.boundaryPositions).toEqual(movedRoots)
    expect(saved.nodes).toHaveLength(1)
    expect(saved.flows).toHaveLength(2)
    expect(saved.flows[0].source).toEqual({ kind: 'boundary' })
    expect(saved.flows[1].target).toEqual({ kind: 'boundary' })
    const rootLeft = (inputRoot.element() as HTMLElement).style.left
    await view.unmount()
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流 版本发布验收', exact: true }).click()
    await openStructure()
    await expect.element(inputRoot).toBeVisible()
    await expect.poll(() => (inputRoot.element() as HTMLElement).style.left).toBe(rootLeft)
    await expect.element(outputRoot).toBeVisible()
    await expect
      .poll(
        () =>
          outputRoot.element().getBoundingClientRect().right -
          document.querySelector('.workflow-canvas')!.getBoundingClientRect().right
      )
      .toBeLessThanOrEqual(0)
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
    definition.nodes = [
      { id: 'prepare', name: '任务拆解', x: 80, y: 330 },
      { id: 'frontend', name: '前端开发', x: 440, y: 160 },
      { id: 'backend', name: '后端开发', x: 390, y: 330 },
      { id: 'tests', name: '测试开发', x: 440, y: 500 },
      { id: 'review', name: '结果评审', x: 760, y: 330 }
    ].map((node) => ({
      ...structuredClone(base),
      ...node,
      modelConfigId: 'model',
      inputRule: { ...base.inputRule, mode: 'all' }
    }))
    definition.boundaryPositions = {
      input: { x: 760, y: 20 },
      output: { x: 1020, y: 20 }
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
      ...forwardFlows,
      {
        id: 'delivery',
        name: '',
        source: { kind: 'node', nodeId: 'review' },
        target: { kind: 'boundary' }
      }
    ]
    definition.viewport = { x: 0, y: 0, zoom: 1 }
    records = [{ definition, revision: 1, updatedAt: 1, issues: [] }]
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流 并行开发与评审', exact: true }).click()
    await openStructure()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    for (const node of definition.nodes)
      await expect
        .element(page.getByRole('group', { name: `节点 ${node.name}`, exact: true }))
        .toBeVisible()
    for (const root of ['根智能体 · 输入', '根智能体 · 输出'])
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
    'distinguishes independent crossing flows with a bridge in the %s theme',
    async (theme) => {
      if (theme === 'dark')
        for (const [key, value] of Object.entries(
          getFrontendCssVariables(undefined, classicDarkTheme)
        ))
          document.documentElement.style.setProperty(key, value)
      const definition = parseWorkflowDefinition(fixture)
      definition.name = '独立任务交叉连线'
      const base = definition.nodes[0]
      definition.nodes = [
        { id: 'horizontal-source', name: '横向任务', x: 80, y: 300 },
        { id: 'horizontal-target', name: '横向交付', x: 820, y: 300 },
        { id: 'vertical-source', name: '纵向任务', x: 200, y: 100 },
        { id: 'vertical-target', name: '纵向交付', x: 700, y: 500 }
      ].map((node) => ({ ...structuredClone(base), ...node, modelConfigId: 'model' }))
      definition.boundaryPositions = {
        input: { x: 80, y: 20 },
        output: { x: 820, y: 20 }
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
      const originalFlows = structuredClone(definition.flows)
      records = [{ definition, revision: 1, updatedAt: 1, issues: [] }]
      await renderWorkflow()
      await page.getByRole('button', { name: '编辑工作流 独立任务交叉连线', exact: true }).click()
      await openStructure()
      await page.getByRole('button', { name: '适应画布', exact: true }).click()
      await page.getByRole('button', { name: '100%', exact: true }).click()
      await expect
        .element(page.getByRole('group', { name: '节点 横向任务', exact: true }))
        .toBeVisible()
      await expect
        .element(page.getByRole('group', { name: '节点 纵向交付', exact: true }))
        .toBeVisible()
      const bridgeGroups = document.querySelectorAll<SVGGElement>('.workflow-crossing')
      expect(bridgeGroups).toHaveLength(1)
      const bridgeGroup = bridgeGroups[0]
      expect(bridgeGroup.dataset.flowId).toBe('vertical')
      const bridge = bridgeGroup.querySelector<SVGPathElement>('.workflow-crossing__line')!
      const halo = bridgeGroup.querySelector<SVGPathElement>('.workflow-crossing__halo')!
      const vertical = page.getByRole('button', { name: '连线 2', exact: true })
      const verticalLine = vertical.element().querySelector<SVGPathElement>('.workflow-edge__line')!
      const originalBridgePath = bridge.getAttribute('d')
      const length = bridge.getTotalLength()
      const start = bridge.getPointAtLength(0)
      const middle = bridge.getPointAtLength(length / 2)
      const end = bridge.getPointAtLength(length)
      expect(middle.x).toBeGreaterThan(Math.max(start.x, end.x))
      expect(bridge.getBBox().width).toBeGreaterThan(0)
      expect(bridge.getBBox().width).toBeLessThan(20)
      const normalStroke = getComputedStyle(bridge).stroke
      expect(normalStroke).toBe(getComputedStyle(verticalLine).stroke)
      expect(getComputedStyle(halo).stroke).toBe(
        getComputedStyle(document.querySelector('.workflow-canvas-shell')!).backgroundColor
      )
      expect(Number.parseFloat(getComputedStyle(halo).strokeWidth)).toBeGreaterThan(
        Number.parseFloat(getComputedStyle(bridge).strokeWidth)
      )
      const originalBridgeWidth = bridge.getBoundingClientRect().width
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/crossing-bridge-${theme}.png`
      })
      ;(vertical.element() as SVGElement).focus()
      await userEvent.keyboard('{Enter}')
      await expect.poll(() => bridgeGroup.classList.contains('is-selected')).toBe(true)
      expect(getComputedStyle(bridge).stroke).toBe(getComputedStyle(verticalLine).stroke)
      expect(getComputedStyle(bridge).stroke).not.toBe(normalStroke)
      for (let index = 0; index < 3; index++)
        await page.getByRole('button', { name: '放大', exact: true }).click()
      await expect
        .poll(() => bridge.getBoundingClientRect().width / originalBridgeWidth)
        .toBeCloseTo(1.3, 2)
      expect(bridge.getAttribute('d')).toBe(originalBridgePath)
      expect(getComputedStyle(bridge).stroke).toBe(getComputedStyle(verticalLine).stroke)
      await page.screenshot({
        path: `../../../../../.cache/workflow-authoring/crossing-bridge-${theme}-selected-zoom.png`
      })
      await page.getByRole('button', { name: '保存工作流', exact: true }).click()
      await expect.poll(() => records[0].revision).toBe(2)
      expect(records[0].definition.flows).toEqual(originalFlows)
    }
  )

  it('shares metadata and graph across tabs and persists the whole workflow with one save', async () => {
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
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
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    expect(records[0].definition.nodes[0]).toMatchObject({
      templateId: null,
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
    await page.getByRole('button', { name: '返回工作流列表', exact: true }).click()
    expect(document.body.textContent).not.toMatch(
      /定义校验|工作流已保存|可复用的协作流程|当前可设计和保存工作流|个节点|条连线/
    )
    await view.unmount()
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流 验证码开发', exact: true }).click()
    await openStructure()
    await expect.element(page.getByRole('group', { name: '节点 开发', exact: true })).toBeVisible()
    await expect
      .element(page.getByRole('group', { name: '节点 开发', exact: true }))
      .toHaveTextContent('模型 B')
  })

  it('undoes and redoes node deletion including its cycle, then protects discarded edits', async () => {
    records = [
      { definition: parseWorkflowDefinition(fixture), revision: 3, updatedAt: 1, issues: [] }
    ]
    await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流 Develop and review', exact: true }).click()
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
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(4)
    expect(records[0].definition.nodes).toHaveLength(2)
    expect(records[0].definition.flows).toHaveLength(4)
    expect(records[0].definition.nodes[1].outputRule.mode).toBe('one')
    await addBlank()
    await page.getByRole('button', { name: '返回工作流列表', exact: true }).click()
    await expect
      .element(page.getByRole('dialog', { name: '放弃未保存的修改？', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '放弃修改', exact: true }).click()
    expect(records[0].definition.nodes).toHaveLength(2)
    await expect
      .element(page.getByRole('button', { name: '编辑工作流 Develop and review', exact: true }))
      .toBeVisible()
  })

  it('adds a template by drag and saves entry, loop, exit and a one-of-two selection group', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('开发和审查')
    await openStructure()
    await addBlank()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('开发')
    await page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }).fill('实现需求。')
    await page.getByRole('button', { name: '连接输入流', exact: true }).click()
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    await page.getByRole('textbox', { name: '搜索模板', exact: true }).fill('审查')
    await expect
      .element(page.getByRole('button', { name: /^审查员模板/ }))
      .toHaveTextContent('模型 A')
    await expect
      .element(page.getByRole('button', { name: /^审查员模板/ }))
      .not.toHaveTextContent('代码审查')
    await page
      .getByRole('button', { name: /^审查员模板/ })
      .dropTo(page.elementLocator(document.querySelector('.workflow-canvas')!), {
        targetPosition: { x: 250, y: 530 }
      })
    expect(document.querySelector('.workflow-graph-inspector')).toBeNull()
    await configureNode('审查员模板')
    await expect
      .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
      .toHaveValue('审查员模板')
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
      .toHaveValue('核实实际问题并附证据。')
    await expect
      .element(page.getByRole('textbox', { name: '模型', exact: true }))
      .toHaveValue('模型 A')
    await expect
      .element(page.getByRole('textbox', { name: '模型', exact: true }))
      .toHaveAttribute('readonly')
    await page
      .getByRole('button', { name: '开发 出流端口', exact: true })
      .dropTo(page.getByRole('button', { name: '审查员模板 入流端口', exact: true }))
    await page.getByRole('button', { name: '审查员模板 出流端口', exact: true }).click()
    await page.getByRole('button', { name: '开发 入流端口', exact: true }).click()
    await configureNode('审查员模板')
    await page.getByRole('button', { name: '连接输出流', exact: true }).click()
    await configureNode('开发')
    await page.getByRole('tab', { name: '流转规则', exact: true }).click()
    await page.getByRole('combobox', { name: '入流规则', exact: true }).selectOptions('one')
    await configureNode('审查员模板')
    await page.getByRole('tab', { name: '流转规则', exact: true }).click()
    await page.getByRole('combobox', { name: '出流规则', exact: true }).selectOptions('custom')
    await page.getByRole('button', { name: '添加分组', exact: true }).click()
    const group = page.getByRole('group', { name: '选择分组 1', exact: true })
    await group.getByRole('checkbox', { name: '审查员模板 → 开发', exact: true }).click()
    await group.getByRole('checkbox', { name: '审查员模板 → 交付主智能体', exact: true }).click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/native-editor-rules.png'
    })
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records.length).toBe(1)
    const definition = records[0].definition
    expect(definition.nodes).toHaveLength(2)
    expect(definition.flows).toHaveLength(4)
    expect(definition.nodes.find((node) => node.name === '开发')?.inputRule.mode).toBe('one')
    const reviewer = definition.nodes.find((node) => node.templateId === 'review-template')!
    expect(reviewer.modelConfigId).toBeNull()
    expect(reviewer.outputRule).toMatchObject({ mode: 'custom', groups: [{ min: 1, max: 1 }] })
    expect(new Set(reviewer.outputRule.groups[0].flowIds)).toEqual(
      new Set(
        definition.flows
          .filter((flow) => flow.source.kind === 'node' && flow.source.nodeId === reviewer.id)
          .map((flow) => flow.id)
      )
    )
    await page.viewport(900, 760)
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await expect.element(page.getByRole('button', { name: '适应画布', exact: true })).toBeVisible()
  })

  it('locks the template model and permits choosing a model after detaching the template', async () => {
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
    await openStructure()
    await addBlank()
    await page.getByRole('textbox', { name: '节点名称', exact: true }).fill('发布验收')
    const canvasNode = page.getByRole('group', { name: '节点 发布验收', exact: true })
    const templatePicker = page.getByRole('button', { name: '子智能体模板', exact: true })
    await page.getByRole('button', { name: '选择模型', exact: true }).click()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    await templatePicker.click()
    const templateOption = page.getByRole('option', { name: /审查员模板/ })
    await expect.element(templateOption).toHaveTextContent('模型 A')
    await expect.element(templateOption).not.toHaveTextContent('代码审查')
    await page.screenshot({ path: '../../../../../.cache/workflow-authoring/template-picker.png' })
    await templateOption.click()
    await expect.element(templatePicker).toHaveFocus()
    await expect
      .element(page.getByRole('textbox', { name: '模型', exact: true }))
      .toHaveValue('模型 A')
    await expect
      .element(page.getByRole('button', { name: '选择模型', exact: true }))
      .not.toBeInTheDocument()
    await expect.element(canvasNode).toHaveTextContent('模型 A')
    await expect.element(canvasNode).not.toHaveTextContent('审查员模板')
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect
      .poll(() => records[0]?.definition.nodes[0])
      .toMatchObject({
        templateId: 'review-template',
        modelConfigId: null
      })
    await templatePicker.click()
    await userEvent.keyboard('{Home}{Enter}')
    await expect.element(templatePicker).toHaveFocus()
    await page.getByRole('button', { name: '选择模型', exact: true }).click()
    await page.getByRole('option', { name: '模型 B', exact: true }).click()
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect
      .poll(() => records[0]?.definition.nodes[0])
      .toMatchObject({
        templateId: null,
        modelConfigId: 'model-b'
      })
    await expect.element(canvasNode).toHaveTextContent('模型 B')
  })

  it('keeps an unavailable saved model until reselected and allows an unconfigured draft', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes[0].modelConfigId = 'retired-model'
    records = [{ definition, revision: 1, updatedAt: 1, issues: [] }]
    const view = await renderWorkflow()
    await page.getByRole('button', { name: '编辑工作流 Develop and review', exact: true }).click()
    await openStructure()
    await configureNode('implement')
    const modelPicker = page.getByRole('button', { name: '选择模型', exact: true })
    await expect.element(modelPicker).toHaveTextContent('旧模型 · 模型不可用')
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records[0].revision).toBe(2)
    expect(records[0].definition.nodes[0].modelConfigId).toBe('retired-model')
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
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
    await openStructure()
    await addBlank()
    await expect.element(modelPicker).toBeDisabled()
    await expect.element(modelPicker).toHaveTextContent('暂无可用模型')
    await expect
      .element(page.getByRole('group', { name: '节点 未命名节点', exact: true }))
      .toHaveTextContent('未选择模型')
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    await expect.poll(() => records[0]?.definition.nodes[0].modelConfigId).toBeNull()
  })

  it('shows save issues only in an acknowledgement dialog and leaves the saved editor ready', async () => {
    await renderWorkflow()
    await page.getByRole('button', { name: '新建工作流', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('草稿流程')
    validationIssues = [
      { code: 'empty', subject: '' },
      { code: 'entry', subject: '' },
      { code: 'exit', subject: '' }
    ]
    await page.getByRole('button', { name: '保存工作流', exact: true }).click()
    const dialog = page.getByRole('alertdialog', { name: '已保存为草稿', exact: true })
    await expect.element(dialog).toHaveTextContent('请添加至少一个节点。')
    await dialog.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(dialog).not.toBeInTheDocument()
    await expect
      .element(page.getByRole('button', { name: '保存工作流', exact: true }))
      .toHaveFocus()
    expect(document.body.textContent).not.toContain('请添加至少一个节点。')
    expect(service.request.mock.calls.map(([request]) => request.operation)).toEqual([
      'list',
      'save'
    ])
    await page.getByRole('button', { name: '返回工作流列表', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '编辑工作流 草稿流程', exact: true }))
      .toBeVisible()
  })
})
