import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowResponse,
  WorkflowRuntimeSnapshot,
  WorkflowRequest
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import type { AppProject } from '../../../config/projectConfig'
import { WORKFLOW_COLORS } from '../../../features/workflows/project/projectWorkflowText'
import {
  begin,
  agent,
  dropConversation,
  createWorkflowInstance,
  createWorkflowPageFixtures,
  createWorkflowPageHarness,
  createWorkflowRequestHandler,
  resetWorkflowPageEnvironment,
  type WorkflowPageTestState
} from './workflowPageTestFixtures'
import '../../../styles/global.css'

const service = vi.hoisted(() => ({
  request: vi.fn(),
  account: 'account-a',
  language: 'zh-CN' as 'zh-CN' | 'en-US',
  runningIds: null as Set<string> | null,
  runtimeListeners: new Set<(snapshot: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: vi.fn((listener: (snapshot: WorkflowRuntimeSnapshot) => void) => {
        service.runtimeListeners.add(listener)
        return () => service.runtimeListeners.delete(listener)
      })
    }
  }
}))
vi.mock('../../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
vi.mock('../../../features/workflows/project/useWorkflowActivity', () => ({
  useWorkflowActivity: (items: WorkflowInstance[]) => ({
    runningInstanceIds:
      service.runningIds ??
      new Set(items.filter((item) => item.enabled && item.running).map((item) => item.id)),
    activityByInstanceId: new Map(items.map((item) => [item.id, item.activity ?? null]))
  })
}))
vi.mock('../../../features/workflows/project/useWorkflowMonitor', () => ({
  useWorkflowMonitor: () => ({
    runningConversationIds: new Set<string>(),
    waitingApprovalConversationIds: new Set<string>()
  })
}))
vi.mock('../../../features/workflows/project/useWorkflowExecution', () => ({
  useWorkflowExecution: () => ({
    snapshot: null,
    transmissions: []
  })
}))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: service.language,
    resolvedColorScheme: 'dark',
    t: (key: string) => key
  })
}))
vi.mock('../../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { status: 'signedIn', profile: { userId: service.account } } })
}))
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      { id: 'node-model', displayName: '节点默认模型', execution: { status: 'available' } },
      { id: 'chat-model', displayName: '已有对话模型', execution: { status: 'available' } },
      { id: 'user-model', displayName: '对话后续模型', execution: { status: 'available' } }
    ],
    models: [
      { id: 'node-model', displayName: '节点默认模型', execution: { status: 'available' } },
      { id: 'chat-model', displayName: '已有对话模型', execution: { status: 'available' } },
      { id: 'user-model', displayName: '对话后续模型', execution: { status: 'available' } }
    ]
  })
}))

const { WorkflowsPage } = await import('../../../features/workflows/project/WorkflowsPage')
const { WorkflowSidebarPage } =
  await import('../../../features/workflows/project/WorkflowSidebarPage')

let record: WorkflowRecord
let extraRecords: WorkflowRecord[]
let instances: WorkflowInstance[]
let conversations: ChatConversation[]
let projects: AppProject[]
let onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>()
let onBeforeCommit = vi.fn<(conversationIds: readonly string[]) => Promise<void>>()

// This suite owns all mutable state; shared helpers only read these live bindings.
const state: WorkflowPageTestState = {
  get record() {
    return record
  },
  get extraRecords() {
    return extraRecords
  },
  get instances() {
    return instances
  },
  get conversations() {
    return conversations
  },
  get projects() {
    return projects
  },
  get onCommitted() {
    return onCommitted
  },
  get onBeforeCommit() {
    return onBeforeCommit
  },
  set instances(value) {
    instances = value
  }
}

const instance = (id: string, name: string, bindings: WorkflowInstance['bindings']) =>
  createWorkflowInstance(record, id, name, bindings)
const { mount } = createWorkflowPageHarness(WorkflowsPage, WorkflowSidebarPage, state, service)

beforeEach(async () => {
  await resetWorkflowPageEnvironment(service)
  ;({ record, conversations, projects } = createWorkflowPageFixtures())
  instances = []
  service.language = 'zh-CN'
  service.runningIds = null
  service.runtimeListeners.clear()
  extraRecords = []
  onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>().mockResolvedValue(undefined)
  onBeforeCommit = vi
    .fn<(conversationIds: readonly string[]) => Promise<void>>()
    .mockResolvedValue(undefined)
  service.request.mockReset().mockImplementation(createWorkflowRequestHandler(state))
})

afterEach(() => vi.useRealTimers())

describe('global organization management', () => {
  it('reuses disabled organization colors but blocks enabling incomplete or conflicting organizations', async () => {
    instances = [
      { ...instance('disabled', '已停用组织', []), enabled: false },
      {
        ...instance('invalid', '需重新确认组织', []),
        enabled: false,
        needsReview: true,
        color: WORKFLOW_COLORS[1]
      }
    ]
    await mount()
    await expect
      .element(page.getByRole('switch', { name: '启用组织 已停用组织', exact: true }))
      .toBeDisabled()
    await begin()
    await page.getByRole('button', { name: '组织颜色', exact: true }).click()
    await expect
      .element(
        page
          .getByRole('dialog', { name: '组织颜色', exact: true })
          .getByRole('button', { name: '标记颜色 1', exact: true })
      )
      .toBeEnabled()
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([request]) => request.operation === 'saveInstance')?.[0]
    ).toMatchObject({ color: WORKFLOW_COLORS[0] })
  })

  it('retains its own color while disabling colors used by other organizations case-insensitively', async () => {
    instances = [
      {
        ...instance('own', '已有配色', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        color: '#4f8fea'
      },
      { ...instance('other', '另一种配色', []), color: '#b57bE8' }
    ]
    await mount()
    await page.getByRole('button', { name: '配置 已有配色', exact: true }).click()
    await page.getByRole('button', { name: '组织颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '组织颜色', exact: true })
    const ownColor = palette.getByRole('button', { name: '标记颜色 1', exact: true })
    const occupiedColor = palette.getByRole('button', { name: '标记颜色 2', exact: true })
    await expect.element(ownColor).toBeEnabled()
    await expect.element(ownColor).toHaveAttribute('aria-pressed', 'true')
    await expect.element(occupiedColor).toBeDisabled()
    ownColor.element().focus()
    await userEvent.keyboard('{ArrowRight}')
    expect(document.activeElement).toBe(
      palette.getByRole('button', { name: '标记颜色 3', exact: true }).element()
    )
    ;(occupiedColor.element() as HTMLButtonElement).click()
    await expect.element(ownColor).toHaveAttribute('aria-pressed', 'true')
    expect(
      service.request.mock.calls.some(([request]) => request.operation === 'saveInstance')
    ).toBe(false)
    await palette.getByRole('button', { name: '标记颜色 3', exact: true }).click()
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([request]) => request.operation === 'saveInstance')?.[0]
    ).toMatchObject({
      id: 'own',
      color: '#28AA91',
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: null }
      ]
    })
  })

  it('chooses an unused default color when creating an organization', async () => {
    instances = [{ ...instance('other', '已使用蓝色', []), color: '#4f8fea' }]
    await mount()
    await begin()
    await page.getByRole('button', { name: '组织颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '组织颜色', exact: true })
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 1', exact: true }))
      .toBeDisabled()
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 2', exact: true }))
      .toHaveAttribute('aria-pressed', 'true')
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([request]) => request.operation === 'saveInstance')?.[0]
    ).toMatchObject({ color: '#B57BE8' })
  })

  it('recovers a server-side color race while retaining staged bindings and blocks another conflicting save locally', async () => {
    await mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    instances = [{ ...instance('concurrent', '刚刚占用颜色', []), color: '#4f8fea' }]
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect
      .element(page.getByRole('alertdialog'))
      .toHaveTextContent('这个颜色已被其他组织使用，请选择其他颜色。')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    expect(instances).toHaveLength(1)
    expect(onCommitted).not.toHaveBeenCalled()
    expect(
      service.request.mock.calls.filter(([request]) => request.operation === 'saveInstance')
    ).toHaveLength(1)
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
    const confirm = page
      .getByRole('button', { name: /^(激活|保存)$/, exact: true })
      .element() as HTMLButtonElement
    confirm.click()
    expect(
      service.request.mock.calls.filter(([request]) => request.operation === 'saveInstance')
    ).toHaveLength(1)
    await page.getByRole('button', { name: '组织颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '组织颜色', exact: true })
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 1', exact: true }))
      .toBeDisabled()
    await palette.getByRole('button', { name: '标记颜色 2', exact: true }).click()
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances).toHaveLength(2)
    const saves = service.request.mock.calls.filter(
      ([request]) => request.operation === 'saveInstance'
    )
    expect(saves).toHaveLength(2)
    expect(saves[1][0]).toMatchObject({
      color: '#B57BE8',
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: null }
      ]
    })
  })

  it('blocks new activation with a clear notice when all colors are used while keeping existing organizations configurable', async () => {
    instances = WORKFLOW_COLORS.map((color, index) => ({
      ...instance(`occupied-${index}`, `颜色流程${index + 1}`, []),
      color: color.toLowerCase()
    }))
    await mount()
    await expect
      .element(page.getByRole('button', { name: '配置 颜色流程1', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '激活新组织', exact: true }).first().click()
    await expect
      .element(page.getByRole('alertdialog'))
      .toHaveTextContent('可选颜色已全部被占用，请先停用或调整其他组织。')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    expect(page.getByRole('textbox', { name: '名称', exact: true }).query()).toBeNull()
    expect(document.querySelector('.project-workflows__template')).toBeNull()
    expect(
      service.request.mock.calls.some(([request]) => request.operation === 'saveInstance')
    ).toBe(false)
    await page.getByRole('button', { name: '配置 颜色流程1', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('颜色流程1')
    await page.getByRole('button', { name: '组织颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '组织颜色', exact: true })
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 1', exact: true }))
      .toBeEnabled()
    for (let index = 2; index <= WORKFLOW_COLORS.length; index++) {
      await expect
        .element(palette.getByRole('button', { name: `标记颜色 ${index}`, exact: true }))
        .toBeDisabled()
    }
  })

  it('collapses organization colors into a single-row palette that closes on selection, outside click and Escape', async () => {
    mount()
    await begin()
    const palette = page.getByRole('button', { name: '组织颜色', exact: true })
    const popover = page.getByRole('dialog', { name: '组织颜色', exact: true })
    expect(page.getByRole('button', { name: '标记颜色 2', exact: true }).query()).toBeNull()
    await palette.click()
    await expect.element(popover).toBeVisible()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/global-workflow-palette-dark.png'
    })
    const swatches = popover
      .getByRole('button')
      .all()
      .map((item) => item.element())
    expect(swatches).toHaveLength(8)
    const top = swatches[0].getBoundingClientRect().top
    for (const swatch of swatches)
      expect(Math.abs(swatch.getBoundingClientRect().top - top)).toBeLessThan(2)
    const firstSwatch = popover.getByRole('button', { name: '标记颜色 1', exact: true }).element()
    firstSwatch.focus()
    firstSwatch.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }))
    expect(document.activeElement).toBe(swatches[7])
    swatches[7].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }))
    expect(document.activeElement).toBe(swatches[0])
    await popover.getByRole('button', { name: '标记颜色 2', exact: true }).click()
    await expect.element(popover).not.toBeInTheDocument()
    await palette.click()
    await expect
      .element(popover.getByRole('button', { name: '标记颜色 2', exact: true }))
      .toHaveAttribute('aria-pressed', 'true')
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await expect.element(popover).not.toBeInTheDocument()
    await palette.click()
    await page.getByRole('textbox', { name: '名称', exact: true }).click()
    await expect.element(popover).not.toBeInTheDocument()
    await dropConversation('需求分析', 'chat-a')
    await expect
      .poll(() => getComputedStyle(agent('需求分析').element()).borderTopColor)
      .toBe('rgb(181, 123, 232)')
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({ color: '#B57BE8' })
  })

  it('saves a disabled organization without activating it when its color is shared', async () => {
    instances = [
      {
        ...instance('disabled', '停用配置', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        enabled: false
      },
      instance('active', '活跃配置', [{ nodeId: 'delivery', conversationId: 'chat-b' }])
    ]
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'saveInstance') {
        instances = instances.map((item) =>
          item.id === input.id ? { ...item, name: input.name, definition: input.definition! } : item
        )
        return { records: [], issues: [], instances }
      }
      return original(input)
    })
    mount()
    await page.getByRole('button', { name: '配置 停用配置' }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('保持停用')
    await expect.element(page.getByRole('button', { name: '保存', exact: true })).toBeEnabled()
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances.find((item) => item.id === 'disabled')?.enabled).toBe(false)
  })
})
