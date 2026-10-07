import type { WorkflowInstance, WorkflowRecord } from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicLightTheme } from '../../config/themes/classic'
import { ToastProvider } from '../../components/toast/ToastProvider'
import '../../styles/global.css'

const service = vi.hoisted(() => ({ request: vi.fn(), openConversation: vi.fn() }))
vi.mock('../../host/hostClient', () => ({
  hostClient: { agent: { onWorkflowRuntimeChanged: vi.fn(() => () => undefined) } }
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
vi.mock('../../features/workflows/project/useWorkflowActivity', () => ({
  useWorkflowActivity: (items: WorkflowInstance[]) => ({
    runningInstanceIds: new Set<string>(),
    activityByInstanceId: new Map(items.map((item) => [item.id, item.activity ?? null]))
  })
}))
vi.mock('../../features/workflows/project/useWorkflowMonitor', () => ({
  useWorkflowMonitor: () => ({
    runningConversationIds: new Set<string>(),
    waitingApprovalConversationIds: new Set<string>()
  })
}))
vi.mock('../../features/workflows/project/useWorkflowExecution', () => ({
  useWorkflowExecution: () => ({
    snapshot: null,
    transmissions: []
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    resolvedColorScheme: 'light',
    t: (key: string) => key
  })
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: null } })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [], enabledModels: [] })
}))

const { WorkflowsPage } = await import('../../features/workflows/project/WorkflowsPage')
const record: WorkflowRecord = {
  enabled: true,
  revision: 1,
  updatedAt: 1,
  issues: [],
  definition: {
    schemaVersion: 1,
    id: 'template',
    name: '文案润色流程',
    description: '',
    background: '',
    nodes: [
      {
        id: 'writer',
        kind: 'agent',
        name: '文书',
        x: 300,
        y: 160,
        modelConfigId: 'model',
        permissionMode: 'default',
        receives: '',
        task: '润色文案',
        delivers: ''
      }
    ],
    viewport: { x: 0, y: 0, zoom: 1 }
  }
}
const instance: WorkflowInstance = {
  id: 'instance',
  name: '文案润色',
  bindings: [{ nodeId: 'writer', conversationId: 'chat' }],
  templateId: record.definition.id,
  templateRevision: 1,
  definition: structuredClone(record.definition),
  revision: 1,
  updatedAt: 1,
  color: '#26AB94',
  enabled: true,
  running: false,
  needsReview: true
}

beforeEach(async () => {
  await page.viewport(1440, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicLightTheme)))
    document.documentElement.style.setProperty(key, value)
  service.request
    .mockReset()
    .mockResolvedValue({ records: [record], instances: [instance], issues: [] })
  service.openConversation.mockReset()
})

function sidebar(width: number) {
  return (
    <ToastProvider>
      <div style={{ width, height: 700, marginLeft: 1440 - width }}>
        <div className="workflow-sidebar-page">
          <WorkflowsPage
            conversations={[
              {
                id: 'chat',
                title: '文书对话',
                projectId: null,
                modelId: 'model',
                messages: [],
                createdAt: 1,
                updatedAt: 1
              }
            ]}
            projects={[]}
            onManageTemplates={vi.fn()}
            onCommitted={vi.fn()}
            onOpenConversation={service.openConversation}
          />
        </div>
      </div>
    </ToastProvider>
  )
}

function expectInsideSidebar(element: Element) {
  const bounds = document.querySelector('.workflow-sidebar-page')!.getBoundingClientRect()
  const rect = element.getBoundingClientRect()
  expect(rect.width).toBeGreaterThan(0)
  expect(rect.left).toBeGreaterThanOrEqual(bounds.left - 1)
  expect(rect.right).toBeLessThanOrEqual(bounds.right + 1)
  expect(rect.top).toBeGreaterThanOrEqual(bounds.top - 1)
  expect(rect.bottom).toBeLessThanOrEqual(bounds.bottom + 1)
}

it('keeps the full elapsed label and actions inside a narrow card with a long organization name', async () => {
  const name = '为多个研究项目持续整理和验证分析结果的组织'
  service.request.mockResolvedValue({
    records: [record],
    instances: [{ ...instance, name, activity: { startedAt: 1_000, completedAt: 94_028_000 } }],
    issues: []
  })
  await render(sidebar(360))
  await expect.element(page.getByText('已连续运行 1d 2h 7m 7s', { exact: true })).toBeVisible()
  const card = document.querySelector<HTMLElement>('.project-workflows__instance')!
  const label = card.querySelector<HTMLElement>('.project-workflows__elapsed')!
  expectInsideSidebar(card)
  expectInsideSidebar(label)
  expect(card.scrollWidth).toBeLessThanOrEqual(card.clientWidth)
  expect(label.scrollWidth).toBeLessThanOrEqual(label.clientWidth)
  for (const button of card.querySelectorAll('button')) expectInsideSidebar(button)
  expect(label.getBoundingClientRect().right).toBeLessThanOrEqual(
    card.querySelector('[role="switch"]')!.getBoundingClientRect().left
  )
})

it.each([360, 560])(
  'keeps home and activation controls reachable in a %ipx sidebar',
  async (width) => {
    await render(sidebar(width))
    await expect
      .element(page.getByRole('button', { name: '配置 文案润色', exact: true }))
      .toBeVisible()
    document
      .querySelectorAll('.project-workflows__header button, .project-workflows__instance button')
      .forEach(expectInsideSidebar)
    await page.getByRole('button', { name: '激活新组织', exact: true }).click()
    await page.getByRole('button', { name: /^模版:/ }).click()
    await page.getByRole('option', { name: '文案润色流程', exact: true }).click()
    document
      .querySelectorAll(
        '.project-workflows__binding-toolbar button, .project-workflows__binding-toolbar input'
      )
      .forEach(expectInsideSidebar)
    const canvas = document.querySelector('.workflow-canvas')!
    expectInsideSidebar(canvas)
    expect(canvas.getBoundingClientRect().height).toBeGreaterThan(300)
    expect(document.querySelector('.project-workflows')!.scrollWidth).toBeLessThanOrEqual(width)
    if (width === 360) {
      await page.screenshot({
        path: '../../../../../.cache/workflow-authoring/organization-toolbar-narrow.png'
      })
      await page.getByRole('group', { name: '节点 文书', exact: true }).dblClick()
      const binding = page.getByRole('button', { name: /^绑定对话 · 文书:/ })
      await expect.element(binding).toBeVisible()
      expectInsideSidebar(binding.element())
      await page.screenshot({
        path: '../../../../../.cache/workflow-authoring/organization-binding-inspector-narrow.png'
      })
    }
  }
)

it('keeps the active organization toolbar on one row at 1040px and colors beside canvas layout', async () => {
  await render(sidebar(1040))
  await page.getByRole('button', { name: '配置 文案润色', exact: true }).click()
  const controls = [
    page.getByRole('textbox', { name: '名称', exact: true }),
    page.getByRole('button', { name: /^所属项目:/ }),
    page.getByRole('tab', { name: '基本信息', exact: true }),
    page.getByRole('tab', { name: '组织设计', exact: true }),
    page.getByRole('button', { name: '取消', exact: true }),
    page.getByRole('button', { name: '保存', exact: true })
  ]
  const first = controls[0].element().getBoundingClientRect()
  for (const control of controls) {
    expectInsideSidebar(control.element())
    const bounds = control.element().getBoundingClientRect()
    expect(Math.abs(bounds.top + bounds.height / 2 - first.top - first.height / 2)).toBeLessThan(2)
  }
  expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
  const palette = page.getByRole('button', { name: '组织颜色', exact: true }).element()
  const layout = page.getByRole('button', { name: '优化布局', exact: true }).element()
  expect(palette.closest('.workflow-canvas-actions')).not.toBeNull()
  expectInsideSidebar(palette)
  expect(palette.getBoundingClientRect().right).toBeLessThan(layout.getBoundingClientRect().left)
  expect(
    Math.abs(palette.getBoundingClientRect().top - layout.getBoundingClientRect().top)
  ).toBeLessThan(2)
  await page.getByRole('button', { name: '组织颜色', exact: true }).click()
  expectInsideSidebar(page.getByRole('dialog', { name: '组织颜色', exact: true }).element())
  await page.screenshot({
    path: '../../../../../.cache/workflow-authoring/organization-toolbar-compact-1040.png'
  })
})

it('keeps the canvas unobstructed after a single click in narrow and wide sidebars', async () => {
  const view = await render(sidebar(360))
  await page.getByRole('button', { name: '组织看板 文案润色', exact: true }).click()
  for (const width of [360, 1100]) {
    await view.rerender(sidebar(width))
    await page.getByRole('button', { name: '双击打开对话 · 文书对话', exact: true }).click()
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(service.openConversation).not.toHaveBeenCalled()
    expect(document.querySelector('.workflow-node-panel')).toBeNull()
    expectInsideSidebar(document.querySelector('.workflow-monitor__canvas')!)
    expectInsideSidebar(document.querySelector('.workflow-canvas-controls')!)
  }
})

it.each([360, 1100])(
  'opens a conversation on the first double-click in a %ipx sidebar',
  async (width) => {
    await render(sidebar(width))
    await page.getByRole('button', { name: '组织看板 文案润色', exact: true }).click()
    await page.getByRole('button', { name: '双击打开对话 · 文书对话', exact: true }).dblClick()
    await expect.poll(() => service.openConversation.mock.calls).toEqual([['chat']])
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(document.querySelector('.workflow-node-panel')).toBeNull()
  }
)
