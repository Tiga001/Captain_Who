import { page } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { HostInvocationError } from '@mycopilot/host-api'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowResponse,
  WorkflowRuntimeSnapshot,
  WorkflowRequest
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import type { AppProject } from '../../../config/projectConfig'
import {
  begin,
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
const { renderPage, mount, SidebarWorkflowHarness } = createWorkflowPageHarness(
  WorkflowsPage,
  WorkflowSidebarPage,
  state,
  service
)

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

describe('organization sidebar adapter', () => {
  it('restores the activity duration from the catalog after a board read that omits historical activity', async () => {
    const full = {
      ...instance('workflow-a', '交付看板', []),
      activity: { startedAt: 1_000, completedAt: 4_000 }
    }
    service.request.mockImplementation(async (input: WorkflowRequest) => ({
      records: [],
      issues: [],
      instances: input.operation === 'getInstance' ? [{ ...full, activity: undefined }] : [full]
    }))
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByRole('heading', { name: '交付看板' })).toBeVisible()
    await screen.getByRole('button', { name: '返回组织', exact: true }).click()
    await expect.element(screen.getByText('已连续运行 3s')).toBeVisible()
    expect(service.request.mock.calls.some(([input]) => input.operation === 'list')).toBe(true)
  })

  it('clears an already mounted board immediately on account change before the new read completes', async () => {
    instances = [instance('workflow-a', '上个账号的组织', [])]
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByRole('heading', { name: '上个账号的组织' })).toBeVisible()
    service.account = 'account-b'
    let resolve!: (response: WorkflowResponse) => void
    service.request.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    await screen.rerender(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByText('正在加载组织…')).toBeVisible()
    expect(screen.container.querySelector('.workflow-monitor__canvas')).toBeNull()
    expect(screen.getByRole('heading', { name: '上个账号的组织' }).query()).toBeNull()
    resolve({ records: [], issues: [], instances: [instance('workflow-a', '重新验证的组织', [])] })
    await expect.element(screen.getByRole('heading', { name: '重新验证的组织' })).toBeVisible()
  })

  it('opens a single organization without loading templates and reuses its snapshot after external navigation', async () => {
    instances = [
      instance('workflow-a', '交付看板', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation !== 'getInstance') throw new Error('Catalog must not block the board')
      return { records: [], issues: [], instances }
    })
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByTestId('workflow-tab-title')).toHaveTextContent('交付看板')
    expect(service.request).toHaveBeenCalledExactlyOnceWith({
      operation: 'getInstance',
      instanceId: 'workflow-a'
    })
    service.request.mockClear()
    service.request.mockImplementation(() => new Promise(() => undefined))
    await screen.rerender(<SidebarWorkflowHarness instanceId="workflow-a" navigationId={2} />)
    await expect.element(screen.getByTestId('workflow-tab-title')).toHaveTextContent('交付看板')
    expect(screen.container.textContent).not.toContain('正在加载组织')
    expect(service.request).toHaveBeenCalledExactlyOnceWith({
      operation: 'getInstance',
      instanceId: 'workflow-a'
    })
  })

  it('shares an unresolved initial read with focus recovery and ends loading when the read fails', async () => {
    let reject!: (error: Error) => void
    service.request.mockImplementation(
      () =>
        new Promise((_resolve, fail) => {
          reject = fail
        })
    )
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByText('正在加载组织…')).toBeVisible()
    window.dispatchEvent(new Event('focus'))
    expect(service.request).toHaveBeenCalledTimes(1)
    reject(new Error('read failed'))
    await expect.element(screen.getByText('暂时无法读取组织')).toBeVisible()
    expect(screen.getByText('组织已不可用').query()).toBeNull()
    await expect.element(screen.getByRole('button', { name: '重试', exact: true })).toBeVisible()
  })

  it('keeps a cached board on rejected revalidation and clears its load error after focus recovery', async () => {
    instances = [
      instance('workflow-a', '交付看板', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect
      .element(screen.getByRole('heading', { name: '交付看板', exact: true }))
      .toBeVisible()
    service.request.mockRejectedValueOnce(
      new HostInvocationError({
        code: -32602,
        message: 'Invalid params: unknown variant `getInstance`, expected one of `list`, `save`',
        data: { privateGraphData: 'do-not-display-payload' }
      })
    )
    await screen.rerender(<SidebarWorkflowHarness instanceId="workflow-a" navigationId={2} />)
    const dialog = screen.getByRole('alertdialog')
    await expect.element(dialog).toHaveTextContent('后台服务无法识别组织读取请求')
    expect(screen.container.querySelector('.workflow-monitor__canvas')).not.toBeNull()
    expect(screen.getByText('组织已不可用').query()).toBeNull()
    expect(dialog.element().textContent).not.toContain('getInstance')
    expect(dialog.element().textContent).not.toContain('do-not-display-payload')
    instances = [{ ...instances[0], name: '已恢复的看板', revision: 2 }]
    window.dispatchEvent(new Event('focus'))
    await expect
      .element(screen.getByRole('heading', { name: '已恢复的看板', exact: true }))
      .toBeVisible()
    await expect.element(dialog).not.toBeInTheDocument()
  })

  it('retains the initial result when focus refresh overlaps its request', async () => {
    let resolve!: (response: WorkflowResponse) => void
    service.request.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        })
    )
    const screen = await render(<SidebarWorkflowHarness instanceId="workflow-a" />)
    await expect.element(screen.getByText('正在加载组织…')).toBeVisible()
    window.dispatchEvent(new Event('focus'))
    resolve({ records: [], issues: [], instances: [instance('workflow-a', '交付看板', [])] })
    await expect.element(screen.getByTestId('workflow-tab-title')).toHaveTextContent('交付看板')
    expect(service.request).toHaveBeenCalledTimes(1)
  })

  it('keeps the live diagram when conversations update and changes the tab title when returning home', async () => {
    instances = [
      instance('workflow-a', '交付看板', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    const openChat = vi.fn()
    const screen = await render(
      <SidebarWorkflowHarness instanceId="workflow-a" onOpenConversation={openChat} />
    )
    await expect.element(screen.getByTestId('workflow-tab-title')).toHaveTextContent('交付看板')
    const canvas = document.querySelector('.workflow-monitor__canvas')
    await screen.rerender(
      <SidebarWorkflowHarness
        instanceId="workflow-a"
        onOpenConversation={openChat}
        chats={conversations.map((chat) => ({ ...chat, updatedAt: 2 }))}
      />
    )
    expect(document.querySelector('.workflow-monitor__canvas')).toBe(canvas)
    await screen
      .getByRole('button', { name: '双击打开对话 · 产品需求讨论', exact: true })
      .dblClick()
    expect(openChat).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(document.querySelector('.workflow-monitor__canvas')).toBe(canvas)
    await screen.getByRole('button', { name: '返回组织', exact: true }).click()
    await expect.element(screen.getByTestId('workflow-tab-title')).toHaveTextContent('组织')
    await expect.element(screen.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
  })

  it('preserves an unfinished configuration across chat updates and resets it on accepted external navigation', async () => {
    const screen = await render(<SidebarWorkflowHarness />)
    await begin()
    await screen.getByRole('textbox', { name: '名称', exact: true }).fill('尚未保存的配置')
    await screen.rerender(<SidebarWorkflowHarness chats={[...conversations]} />)
    await expect
      .element(screen.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('尚未保存的配置')
    await screen.rerender(<SidebarWorkflowHarness navigationId={2} />)
    await expect.element(screen.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
    expect(screen.container.querySelector('.project-workflows__binding-toolbar')).toBeNull()
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
  })
})

describe('global organization management', () => {
  it('opens a separate read-only diagram before the configuration action and opens its bound conversation', async () => {
    instances = [
      instance('workflow-a', '交付看板', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    const onOpenConversation = vi.fn()
    const onMonitorChange = vi.fn()
    await mount({
      onOpenConversation,
      onMonitorChange,
      conversationAttention: {
        'chat-a': { waitingApproval: false, waitingAnswer: true, unread: true }
      }
    })
    const diagram = page.getByRole('button', { name: '组织看板 交付看板', exact: true })
    await expect.element(diagram).toBeVisible()
    const configure = page.getByRole('button', { name: '配置 交付看板', exact: true }).element()
    expect(
      diagram.element().compareDocumentPosition(configure) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    await diagram.click()
    await expect.element(page.getByRole('button', { name: '返回组织', exact: true })).toBeVisible()
    expect(document.querySelector('.project-workflows__library')).toBeNull()
    expect(document.querySelector('.workflow-binding-node')).toBeNull()
    await expect.element(page.getByText('等待交互', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '双击打开对话 · 产品需求讨论', exact: true }).dblClick()
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(onMonitorChange).toHaveBeenCalledWith('workflow-a')
    expect(
      service.request.mock.calls.every(([input]) =>
        ['list', 'listInstances', 'getInstance'].includes(input.operation)
      )
    ).toBe(true)
    await page.getByRole('button', { name: '返回组织', exact: true }).click()
    await expect.element(diagram).toBeVisible()
    expect(onMonitorChange).toHaveBeenLastCalledWith(null)
  })

  it('opens the requested diagram from a conversation and switches targets without changing bindings', async () => {
    instances = [
      instance('workflow-a', '交付看板', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
      {
        ...instance('workflow-b', '停用看板', [{ nodeId: 'analysis', conversationId: 'chat-b' }]),
        enabled: false
      }
    ]
    const onOpenConversation = vi.fn()
    const view = await mount({ initialMonitorId: 'workflow-a', onOpenConversation })
    await expect
      .element(page.getByRole('button', { name: '双击打开对话 · 产品需求讨论', exact: true }))
      .toBeVisible()
    await view.rerender(renderPage({ initialMonitorId: 'workflow-b', onOpenConversation }))
    await page.getByRole('button', { name: '双击打开对话 · 界面设计讨论', exact: true }).dblClick()
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-b')
    expect(document.querySelector('.project-workflows__library')).toBeNull()
    await view.rerender(renderPage({ initialMonitorId: null, onOpenConversation }))
    await expect
      .element(page.getByRole('button', { name: '组织看板 停用看板', exact: true }))
      .toBeVisible()
  })

  it('keeps a missing diagram recoverable instead of showing another organization', async () => {
    await mount({ initialMonitorId: 'missing' })
    await expect.element(page.getByText('组织已不可用', { exact: true })).toBeVisible()
    await expect.element(page.getByRole('button', { name: '重试', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '返回组织', exact: true }).click()
    await expect.element(page.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
  })

  it('automatically retries a background read failure while leaving the current board available', async () => {
    instances = [
      instance('workflow-a', '交付看板', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    await mount({ initialMonitorId: 'workflow-a' })
    const conversation = page.getByRole('button', {
      name: '双击打开对话 · 产品需求讨论',
      exact: true
    })
    await expect.element(conversation).toBeVisible()
    const listReads = () =>
      service.request.mock.calls.filter(([input]) => input.operation === 'getInstance').length
    const before = listReads()
    instances = [{ ...instances[0], name: '自动恢复的看板', revision: 2 }]
    service.request.mockRejectedValueOnce(new Error('Invalid organization fields'))
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.element(conversation).toBeVisible()
    expect(page.getByRole('alertdialog').query()).toBeNull()
    await expect.poll(listReads).toBe(before + 2)
    expect(document.body.textContent).not.toContain('Invalid organization fields')
    await expect.element(conversation).toBeVisible()
    await page.getByRole('button', { name: '返回组织', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '组织看板 自动恢复的看板', exact: true }))
      .toBeVisible()
    expect(page.getByRole('alertdialog').query()).toBeNull()
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
  })
})
