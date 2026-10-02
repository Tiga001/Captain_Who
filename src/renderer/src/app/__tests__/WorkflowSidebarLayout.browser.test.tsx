import type { WorkflowInstance, WorkflowRecord } from '@mycopilot/protocol'
import { page } from 'vitest/browser'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicLightTheme } from '../../config/themes/classic'
import '../../styles/global.css'

const service = vi.hoisted(() => ({ request: vi.fn(), openConversation: vi.fn() }))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
vi.mock('../../features/workflows/project/useWorkflowActivity', () => ({
  useWorkflowActivity: () => new Set<string>()
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
    transmissions: [],
    completeUserInput: vi.fn()
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', resolvedColorScheme: 'light' })
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: null } })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [] })
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
    await page.getByRole('button', { name: '激活新工作流', exact: true }).click()
    await page.getByRole('button', { name: /^模版:/ }).click()
    await page.getByRole('option', { name: '文案润色流程', exact: true }).click()
    document
      .querySelectorAll(
        '.project-workflows__binding-toolbar button, .project-workflows__binding-toolbar input'
      )
      .forEach(expectInsideSidebar)
    const canvas = document.querySelector('.workflow-binding-canvas')!
    expectInsideSidebar(canvas)
    expect(canvas.getBoundingClientRect().height).toBeGreaterThan(300)
    expect(document.querySelector('.project-workflows')!.scrollWidth).toBeLessThanOrEqual(width)
  }
)

it('contains the node dashboard in the sidebar and restores the unobstructed canvas when closed', async () => {
  const view = await render(sidebar(360))
  await page.getByRole('button', { name: '工作流看板 文案润色', exact: true }).click()
  await page.getByRole('button', { name: '双击打开对话 · 文书对话', exact: true }).click()
  const close = page.getByRole('button', { name: '收起节点看板', exact: true })
  await expect.element(close).toBeVisible()
  expect(service.openConversation).not.toHaveBeenCalled()
  expectInsideSidebar(close.element())
  expectInsideSidebar(document.querySelector('.workflow-node-panel')!)
  expect(getComputedStyle(document.querySelector('.workflow-node-panel')!).position).toBe(
    'absolute'
  )
  await view.rerender(sidebar(1100))
  await expect
    .poll(() => getComputedStyle(document.querySelector('.workflow-node-panel')!).position)
    .toBe('static')
  expectInsideSidebar(document.querySelector('.workflow-node-panel')!)
  expect(
    document.querySelector('.workflow-node-panel')!.getBoundingClientRect().left
  ).toBeGreaterThanOrEqual(
    document.querySelector('.workflow-monitor__canvas')!.getBoundingClientRect().right
  )
  await view.rerender(sidebar(360))
  await close.click()
  expect(document.querySelector('.workflow-node-panel')).toBeNull()
  expectInsideSidebar(document.querySelector('.workflow-monitor__canvas')!)
  expectInsideSidebar(document.querySelector('.workflow-canvas-controls')!)
})

it.each([360, 1100])(
  'opens a conversation on the first double-click in a %ipx sidebar',
  async (width) => {
    await render(sidebar(width))
    await page.getByRole('button', { name: '工作流看板 文案润色', exact: true }).click()
    await page.getByRole('button', { name: '双击打开对话 · 文书对话', exact: true }).dblClick()
    await expect.poll(() => service.openConversation.mock.calls).toEqual([['chat']])
    await new Promise((resolve) => setTimeout(resolve, 400))
    expect(document.querySelector('.workflow-node-panel')).toBeNull()
  }
)
