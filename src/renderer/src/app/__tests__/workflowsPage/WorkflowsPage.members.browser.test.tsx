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
import {
  deferred,
  begin,
  agent,
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
const { mount, membersChanged } = createWorkflowPageHarness(
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

describe('global organization management', () => {
  it('edits member tasks and rank with the shared configuration panel', async () => {
    mount()
    await begin()
    await userEvent.dblClick(agent('需求分析'))
    await expect
      .element(page.getByRole('complementary', { name: '节点配置', exact: true }))
      .toBeVisible()
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('梳理全部需求')
    await page.getByRole('tab', { name: '职级', exact: true }).click()
    await page.getByRole('spinbutton', { name: '职级', exact: true }).fill('4')
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances[0].definition.nodes[0]).toMatchObject({ task: '梳理全部需求', rank: 4 })
  })

  it('supports adding, deleting and restoring members with layout and zoom controls in the live editor', async () => {
    instances = [
      instance('existing', '独立组织', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    // The instance retains its independent definition even after its source template disappears.
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'saveInstance') {
        instances = [{ ...instances[0], definition: input.definition!, revision: 2 }]
      }
      return { records: [], issues: [], instances }
    })
    mount()
    await page.getByRole('button', { name: '配置 独立组织' }).click()
    await expect.element(agent('需求分析')).toBeVisible()
    expect(page.getByRole('button', { name: /^模版:/ }).query()).toBeNull()
    await page.getByRole('button', { name: '添加节点', exact: true }).click()
    await expect.element(page.getByRole('button', { name: '添加部门', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '新建智能体', exact: true }).click()
    await expect.poll(() => document.querySelectorAll('[data-workflow-node-id]').length).toBe(3)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect.poll(() => document.querySelectorAll('[data-workflow-node-id]').length).toBe(2)
    await page.getByRole('button', { name: '重做', exact: true }).click()
    await expect.poll(() => document.querySelectorAll('[data-workflow-node-id]').length).toBe(3)
    await page.getByRole('button', { name: '优化布局', exact: true }).click()
    await page.getByRole('button', { name: '放大', exact: true }).click()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await page.screenshot({ path: '../../../../../../.cache/organizations/live-editor.png' })
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances[0].definition.nodes).toHaveLength(3)
  })

  it('edits public context and refreshes remote members and bindings while retaining local edits', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    mount()
    await page.getByRole('button', { name: '配置 独立组织' }).click()
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    await page.getByRole('textbox', { name: '简短描述', exact: true }).fill('本地说明')
    await page.getByRole('textbox', { name: '组织公共背景', exact: true }).fill('共享产品背景')
    await page.screenshot({ path: '../../../../../../.cache/organizations/context-editor.png' })
    await page.getByRole('tab', { name: '组织设计', exact: true }).click()
    const editorTop = () =>
      document.querySelector('.project-workflows__binding-content')!.getBoundingClientRect().top
    const initialEditorTop = editorTop()
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
    const updated = structuredClone(instances[0])
    updated.revision = 2
    updated.definition.nodes[1].name = '远端改名成员'
    updated.bindings[0].conversationId = 'chat-c'
    instances = [updated]
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.element(agent('远端改名成员')).toBeVisible()
    expect(editorTop()).toBe(initialEditorTop)
    expect(page.getByRole('status').query()).toBeNull()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/organization-automatic-update.png'
    })
    await expect.element(agent('需求分析')).toHaveTextContent('自动化验收讨论')
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '简短描述', exact: true }))
      .toHaveValue('本地说明')
    await expect
      .element(page.getByRole('textbox', { name: '组织公共背景', exact: true }))
      .toHaveValue('共享产品背景')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances[0].definition).toMatchObject({
      description: '本地说明',
      background: '共享产品背景'
    })
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({
      expectedRevision: 2,
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-c' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ]
    })
  })

  it('recovers member changes from a durable summary revision outside the event window', async () => {
    instances = [instance('existing', '原组织', [])]
    await mount()
    await expect.element(page.getByText('原组织', { exact: true })).toBeVisible()
    instances = [{ ...instances[0], name: '恢复后的组织', revision: 2 }]
    const snapshot: WorkflowRuntimeSnapshot = {
      instanceId: 'existing',
      sequence: 700,
      inputs: [],
      events: [],
      summary: { pendingByNode: [], conversationChanges: [], structureRevision: 42 }
    }
    service.runtimeListeners.forEach((listener) => listener(snapshot))
    await expect.element(page.getByText('恢复后的组织', { exact: true })).toBeVisible()
    const reads = service.request.mock.calls.length
    service.runtimeListeners.forEach((listener) => listener(snapshot))
    expect(service.request).toHaveBeenCalledTimes(reads)
  })

  it('preserves edits made during automatic synchronization and applies a later member notification', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    await mount()
    await page.getByRole('button', { name: '配置 独立组织', exact: true }).click()
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    const description = page.getByRole('textbox', { name: '简短描述', exact: true })
    await description.fill('同步前的编辑')
    const firstRead = deferred()
    const secondRead = deferred()
    const original = service.request.getMockImplementation()!
    let reads = 0
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation !== 'list') return original(input)
      const snapshot = structuredClone(instances)
      const read = ++reads
      if (read === 1) await firstRead.promise
      if (read === 2) await secondRead.promise
      return { records: [record], issues: [], instances: snapshot }
    })
    const firstUpdate = structuredClone(instances[0])
    firstUpdate.revision = 2
    firstUpdate.definition.nodes[1].name = '第一次远端改名'
    instances = [firstUpdate]
    membersChanged('existing', 42)
    await expect.poll(() => reads).toBe(1)
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('同步期间编辑的组织名')
    await description.fill('第一个请求尚未返回时编辑')
    const secondUpdate = structuredClone(firstUpdate)
    secondUpdate.revision = 3
    secondUpdate.definition.nodes[1].name = '最终远端成员'
    instances = [secondUpdate]
    membersChanged('existing', 43)
    firstRead.resolve()
    await expect.poll(() => reads).toBe(2)
    await expect.element(description).toHaveValue('第一个请求尚未返回时编辑')
    await description.fill('第二个请求尚未返回时编辑')
    secondRead.resolve()
    await page.getByRole('tab', { name: '组织设计', exact: true }).click()
    await expect.element(agent('最终远端成员')).toBeVisible()
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    await expect.element(description).toHaveValue('第二个请求尚未返回时编辑')
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('同步期间编辑的组织名')
    membersChanged('existing', 43)
    expect(reads).toBe(2)
    await description.fill('同版本通知到达前的临时编辑')
    membersChanged('existing', 44)
    await expect.poll(() => reads).toBe(3)
    await expect.element(page.getByRole('button', { name: '撤销', exact: true })).toBeEnabled()
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    await expect.element(description).toHaveValue('第二个请求尚未返回时编辑')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({
      expectedRevision: 3,
      name: '同步期间编辑的组织名',
      definition: {
        description: '第二个请求尚未返回时编辑',
        nodes: [{ id: 'analysis' }, { id: 'delivery', name: '最终远端成员' }]
      }
    })
  })

  it('does not recreate remotely removed members when refreshing local task edits and saving', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    mount()
    await page.getByRole('button', { name: '配置 独立组织' }).click()
    await userEvent.dblClick(agent('需求分析'))
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('本地任务修改')
    await page.getByRole('tab', { name: '基本信息', exact: true }).click()
    await page.getByRole('textbox', { name: '简短描述', exact: true }).fill('保留组织说明')
    const updated = structuredClone(instances[0])
    updated.revision = 2
    updated.definition.nodes = updated.definition.nodes.filter((node) => node.id !== 'analysis')
    updated.bindings = updated.bindings.filter((binding) => binding.nodeId !== 'analysis')
    instances = [updated]
    membersChanged(updated.id, 42)
    await expect
      .element(page.getByRole('dialog', { name: '部分修改需要检查', exact: true }))
      .toHaveTextContent('“需求分析”已被移除，相关修改未恢复；其余修改已保留，请检查后保存。')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '简短描述', exact: true }))
      .toHaveValue('保留组织说明')
    await page.getByRole('tab', { name: '组织设计', exact: true }).click()
    expect(agent('需求分析').query()).toBeNull()
    await expect.element(agent('开发交付')).toHaveTextContent('界面设计讨论')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const saved = service.request.mock.calls.find(
      ([input]) => input.operation === 'saveInstance'
    )?.[0]
    expect(saved).toMatchObject({
      expectedRevision: 2,
      bindings: [{ nodeId: 'delivery', conversationId: 'chat-b' }],
      definition: { description: '保留组织说明', nodes: [{ id: 'delivery' }] }
    })
    expect(instances[0].bindings).toEqual([{ nodeId: 'delivery', conversationId: 'chat-b' }])
    expect(service.request.mock.calls.some(([input]) => input.operation === 'listInstances')).toBe(
      false
    )
  })

  it('refreshes a member change received while an availability change is still synchronizing', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    const gate = deferred()
    onCommitted.mockImplementationOnce(() => gate.promise)
    mount()
    await page.getByRole('switch', { name: '停用组织 独立组织', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const listReads = () =>
      service.request.mock.calls.filter(([input]) => input.operation === 'list').length
    const before = listReads()
    instances = [{ ...instances[0], name: '远端更新组织', revision: 3 }]
    membersChanged('existing', 42)
    membersChanged('existing', 43)
    expect(listReads()).toBe(before)
    gate.resolve()
    await expect
      .element(page.getByRole('button', { name: '配置 远端更新组织', exact: true }))
      .toBeVisible()
    expect(listReads()).toBe(before + 1)
    membersChanged('existing', 43)
    expect(listReads()).toBe(before + 1)
  })

  it('refreshes a member change received after save commits but before its synchronization finishes', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    const gate = deferred()
    onCommitted.mockImplementationOnce(() => gate.promise)
    mount()
    await page.getByRole('button', { name: '配置 独立组织', exact: true }).click()
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const before = service.request.mock.calls.length
    const updated = structuredClone(instances[0])
    updated.name = '保存后变更组织'
    updated.revision = 3
    updated.definition.nodes[0].name = '保存后变更成员'
    instances = [updated]
    membersChanged('existing', 42)
    expect(service.request.mock.calls).toHaveLength(before)
    gate.resolve()
    await expect
      .element(page.getByRole('button', { name: '配置 保存后变更组织', exact: true }))
      .toBeEnabled()
    await page.getByRole('button', { name: '配置 保存后变更组织', exact: true }).click()
    await expect.element(agent('保存后变更成员')).toBeVisible()
    expect(service.request.mock.calls.slice(before).map(([input]) => input.operation)).toEqual([
      'list'
    ])
  })

  it('defers member notifications through a failed synchronization and refreshes after its retry', async () => {
    instances = [
      instance('existing', '独立组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    onCommitted.mockRejectedValueOnce(new Error('refresh unavailable'))
    mount()
    await page.getByRole('switch', { name: '停用组织 独立组织', exact: true }).click()
    await expect.element(page.getByRole('alertdialog')).toHaveTextContent('组织已保存')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    const before = service.request.mock.calls.length
    instances = [{ ...instances[0], name: '同步后的组织', revision: 3 }]
    membersChanged('existing', 42)
    expect(service.request.mock.calls).toHaveLength(before)
    await page.getByRole('button', { name: '重试同步', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '配置 同步后的组织', exact: true }))
      .toBeVisible()
    expect(service.request.mock.calls.slice(before).map(([input]) => input.operation)).toEqual([
      'list'
    ])
    expect(onCommitted).toHaveBeenCalledTimes(2)
  })
})
