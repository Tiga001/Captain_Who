import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowResponse,
  WorkflowRuntimeSnapshot,
  WorkflowRequest
} from '@mycopilot/protocol'
import type { ChatConversation, ChatComposerDraft } from '../../../features/chat/chatTypes'
import type { AppProject } from '../../../config/projectConfig'
import { WORKFLOW_COLORS } from '../../../features/workflows/project/projectWorkflowText'
import {
  begin,
  chooseTemplate,
  agent,
  dropConversation,
  expectBoundCount,
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
const { renderPage, mount } = createWorkflowPageHarness(
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
  it('opens an empty canvas directly and requires a ready template before confirmation', async () => {
    extraRecords = [
      {
        ...record,
        enabled: false,
        definition: { ...record.definition, id: 'disabled', name: '未就绪模板' }
      }
    ]
    await mount()
    await page.getByRole('button', { name: '激活新组织', exact: true }).first().click()
    await expect.element(page.getByLabelText('空白组织画布')).toBeVisible()
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeDisabled()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/global-workflow-empty-canvas-dark.png'
    })
    expect(document.querySelector('.project-workflows__template')).toBeNull()
    expect(agent('需求分析').query()).toBeNull()
    await page.getByRole('button', { name: /^模版:/ }).click()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/global-workflow-template-menu-dark.png'
    })
    expect(page.getByRole('option', { name: '未就绪模板', exact: true }).query()).toBeNull()
    await page.getByRole('option', { name: '产品交付流程', exact: true }).click()
    await expect.element(agent('需求分析')).toBeVisible()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('产品交付流程')
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeEnabled()
    const select = page
      .getByRole('button', { name: /^模版:/ })
      .element()
      .getBoundingClientRect()
    const name = page
      .getByRole('textbox', { name: '名称', exact: true })
      .element()
      .getBoundingClientRect()
    expect(select.right).toBeLessThan(name.left)
    await page.getByRole('button', { name: '取消', exact: true }).click()
    expect(page.getByRole('dialog').query()).toBeNull()
  })

  it('confirms template replacement before clearing staged bindings and fixes saved instance templates', async () => {
    extraRecords = [
      {
        ...record,
        definition: {
          ...record.definition,
          id: 'template-b',
          name: '另一套流程',
          nodes: record.definition.nodes.map((node) => ({ ...node, id: `new-${node.id}` }))
        }
      }
    ]
    await mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    await chooseTemplate('另一套流程')
    await expect.element(page.getByRole('dialog')).toHaveTextContent('放弃未保存的修改')
    await page.getByRole('button', { name: '继续编辑', exact: true }).last().click()
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    await chooseTemplate('另一套流程')
    await page.getByRole('button', { name: '放弃修改', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('另一套流程')
    await expectBoundCount(0)
    expect(onBeforeCommit).not.toHaveBeenCalled()
    await page.getByRole('button', { name: '取消', exact: true }).click()
    instances = [
      instance('existing', '已配置组织', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await page.getByRole('button', { name: '配置 已配置组织', exact: true }).click()
    expect(page.getByRole('button', { name: /^模版:/ }).query()).toBeNull()
  })

  it('preserves a disabled organization when its configuration is saved', async () => {
    instances = [
      {
        ...instance('paused', '已停用组织', [
          { nodeId: 'analysis', conversationId: 'chat-a' },
          { nodeId: 'delivery', conversationId: 'chat-b' }
        ]),
        enabled: false
      }
    ]
    await mount()
    await page.getByRole('button', { name: '配置 已停用组织', exact: true }).click()
    expect(page.getByRole('button', { name: '模版: 产品交付流程', exact: true }).query()).toBeNull()
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      onCommitted.mock.calls[0][0].instances?.find((item) => item.id === 'paused')?.enabled
    ).toBe(false)
    await expect
      .element(page.getByRole('switch', { name: '启用组织 已停用组织', exact: true }))
      .toHaveAttribute('aria-checked', 'false')
  })

  it('stages cross-project bindings and cancels without modifying real conversations', async () => {
    const onDirtyChange = vi.fn()
    mount({ onDirtyChange })
    await begin()
    await dropConversation('需求分析', 'chat-b')
    await expect.element(agent('需求分析')).toHaveTextContent('界面设计讨论')
    expect(page.getByRole('complementary', { name: '选择对话', exact: true }).query()).toBeNull()
    expect(page.getByRole('textbox', { name: '搜索对话', exact: true }).query()).toBeNull()
    await expectBoundCount(1)
    expect(onDirtyChange).toHaveBeenLastCalledWith(true)
    expect(
      service.request.mock.calls.every(
        ([input]) => input.operation === 'list' || input.operation === 'listInstances'
      )
    ).toBe(true)
    await page.getByRole('button', { name: '取消', exact: true }).click()
    await expect.element(page.getByRole('dialog')).toBeVisible()
    await page.getByRole('button', { name: '放弃修改' }).click()
    await expect.element(page.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
    expect(onBeforeCommit).not.toHaveBeenCalled()
    expect(onCommitted).not.toHaveBeenCalled()
  })

  it('edits organization member defaults while preserving the conversation composer until save', async () => {
    instances = [
      instance('existing', '已配置组织', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    const composerDraft: ChatComposerDraft = {
      message: '尚未发送',
      permissionMode: 'full',
      modelId: 'user-model',
      projectId: 'project-a',
      attachments: [],
      skills: [],
      queuedMessages: [],
      updatedAt: 2
    }
    mount({ conversationDrafts: { 'chat-a': composerDraft } })
    await page.getByRole('button', { name: '配置 已配置组织' }).click()
    await userEvent.dblClick(agent('需求分析'))
    await page
      .getByRole('textbox', { name: '这个节点需要做什么', exact: true })
      .fill('更新后的需求分析')
    expect(composerDraft.modelId).toBe('user-model')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances[0].definition.nodes[0].task).toBe('更新后的需求分析')
    expect(record.definition.nodes[0].task).toBe('分析需求')
  })

  it('submits existing conversation IDs and unbound nodes once after flushing pending conversation writes', async () => {
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(onBeforeCommit).toHaveBeenCalledWith(['chat-a'])
    const saves = service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    expect(saves).toHaveLength(1)
    expect(saves[0][0]).toMatchObject({
      templateId: 'template-a',
      expectedRevision: 0,
      expectedTemplateRevision: 1,
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: null }
      ]
    })
    expect(saves[0][0]).toHaveProperty('projectId', null)
    expect(onBeforeCommit.mock.invocationCallOrder[0]).toBeLessThan(
      service.request.mock.invocationCallOrder.at(-1)!
    )
    await expect
      .element(page.getByRole('button', { name: '配置 产品交付流程', exact: true }))
      .toBeVisible()
  })

  it('selects a default project for new conversations while accepting bindings from another project', async () => {
    await mount()
    await begin()
    await page.getByRole('button', { name: '所属项目: 无项目', exact: true }).click()
    await page.getByRole('option', { name: '产品项目', exact: true }).click()
    await dropConversation('需求分析', 'chat-b')
    await expectBoundCount(1)
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const save = service.request.mock.calls.find(
      ([input]) => input.operation === 'saveInstance'
    )?.[0]
    expect(save).toMatchObject({
      projectId: 'project-a',
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-b' },
        { nodeId: 'delivery', conversationId: null }
      ]
    })
    expect(conversations.find((item) => item.id === 'chat-b')?.projectId).toBe('project-b')
    await page.getByRole('button', { name: '配置 产品交付流程', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '所属项目: 产品项目', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '所属项目: 产品项目', exact: true }).click()
    await page.getByRole('option', { name: '无项目', exact: true }).click()
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(2)
    const saves = service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    expect(saves[1][0]).toHaveProperty('projectId', null)
  })

  it('keeps template, name, project and actions on a compact toolbar and blocks deleted project choices', async () => {
    await page.viewport(1440, 900)
    const view = await mount()
    await begin()
    await page.getByRole('button', { name: '所属项目: 无项目', exact: true }).click()
    await page.getByRole('option', { name: '产品项目', exact: true }).click()
    await dropConversation('需求分析', 'chat-b')
    const controls = [
      page.getByRole('button', { name: /^模版:/ }),
      page.getByRole('textbox', { name: '名称', exact: true }),
      page.getByRole('button', { name: /^所属项目:/ }),
      page.getByRole('tab', { name: '基本信息', exact: true }),
      page.getByRole('tab', { name: '组织设计', exact: true }),
      page.getByRole('button', { name: '取消', exact: true }),
      page.getByRole('button', { name: /^(激活|保存)$/, exact: true })
    ].map((control) => control.element().getBoundingClientRect())
    for (const [index, rect] of controls.entries()) {
      expect(
        Math.abs(rect.top + rect.height / 2 - controls[0].top - controls[0].height / 2)
      ).toBeLessThan(2)
      expect(rect.left).toBeGreaterThanOrEqual(0)
      expect(rect.right).toBeLessThanOrEqual(1440)
      if (index > 0) expect(rect.left).toBeGreaterThan(controls[index - 1].right)
    }
    expect(document.querySelector('.project-workflows__editor-tabs')).toBeNull()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/workflow-project-toolbar-dark.png'
    })
    await page.viewport(640, 700)
    await expect.element(page.getByRole('button', { name: /^模版:/ })).toBeVisible()
    await expect.element(page.getByRole('button', { name: /^所属项目:/ })).toBeVisible()
    expect(document.querySelector('.project-workflows')!.scrollWidth).toBeLessThanOrEqual(640)
    await view.rerender(
      renderPage({ projects: projects.filter((project) => project.id !== 'project-a') })
    )
    await expect
      .element(page.getByRole('button', { name: '所属项目: 项目已删除', exact: true }))
      .toBeVisible()
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeDisabled()
    await page.getByRole('button', { name: '所属项目: 项目已删除', exact: true }).click()
    await page.getByRole('option', { name: '无项目', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeEnabled()
  })

  it('prevents a conversation being assigned to a second organization or a second node, including drop actions', async () => {
    instances = [
      instance('other', '另一组组织', [{ nodeId: 'analysis', conversationId: 'chat-b' }])
    ]
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-b')
    await expect.element(page.getByRole('dialog')).toHaveTextContent('已经加入其他组织“另一组组织”')
    expect(page.getByRole('alert').query()).toBeNull()
    expect(page.getByRole('button', { name: '刷新版本' }).query()).toBeNull()
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/global-workflow-assignment-conflict.png'
    })
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expectBoundCount(0)
    await dropConversation('需求分析', 'chat-a')
    await dropConversation('开发交付', 'chat-a')
    await expect.element(page.getByRole('dialog')).toHaveTextContent('已分配给当前组织的“需求分析”')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expectBoundCount(1)
    await dropConversation('开发交付', 'missing-chat')
    await expect.element(page.getByRole('alertdialog')).toHaveTextContent('这个对话当前不可用')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await dropConversation('开发交付', 'chat-c')
    await expectBoundCount(2)
    expect(page.getByRole('alert').query()).toBeNull()
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
  })

  it('shows a single acknowledgement dialog for a backend assignment race and preserves the draft', async () => {
    await mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    const name = page.getByRole('textbox', { name: '名称', exact: true })
    await name.fill('保留当前配置')
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'saveInstance') {
        instances = [
          {
            ...instance('other', '先完成绑定的流程', [
              { nodeId: 'analysis', conversationId: 'chat-a' }
            ]),
            color: WORKFLOW_COLORS[1]
          }
        ]
        throw new Error('workflow_conversation_already_bound')
      }
      return original(input)
    })
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    const dialog = page.getByRole('dialog')
    await expect.element(dialog).toHaveTextContent('这个对话已经加入其他组织')
    expect(dialog.element().querySelectorAll('.app-confirm-dialog__actions button')).toHaveLength(1)
    expect(page.getByRole('alert').query()).toBeNull()
    expect(page.getByRole('button', { name: '刷新版本' }).query()).toBeNull()
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(name).toHaveValue('保留当前配置')
    await expectBoundCount(1)
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    expect(onCommitted).not.toHaveBeenCalled()
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    ).toHaveLength(1)
  })

  it('keeps successful activation committed when sidebar refresh fails, then retries only synchronization', async () => {
    onCommitted
      .mockRejectedValueOnce(new Error('Storage refresh failed'))
      .mockResolvedValue(undefined)
    mount()
    await begin()
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.element(page.getByRole('alertdialog')).toHaveTextContent('组织已保存')
    expect(document.body.textContent).not.toContain('Storage refresh failed')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(page.getByRole('button', { name: '重试同步' })).toBeVisible()
    await page.getByRole('button', { name: '重试同步' }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(2)
    expect(onCommitted.mock.calls[0][0]).toEqual(onCommitted.mock.calls[1][0])
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    ).toHaveLength(1)
  })

  it('acknowledges a save failure without exposing diagnostics or losing staged assignments', async () => {
    await mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    const name = page.getByRole('textbox', { name: '名称', exact: true })
    await name.fill('保留未保存组织')
    const original = service.request.getMockImplementation()!
    let fail = true
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'saveInstance' && fail) throw new Error('Invalid organization fields')
      return original(input)
    })
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    const dialog = page.getByRole('alertdialog', { name: '暂时无法完成操作' })
    await expect.element(dialog).toHaveTextContent('暂时无法保存组织')
    expect(document.body.textContent).not.toContain('Invalid organization fields')
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/organization-home-save-error-dialog.png'
    })
    expect(dialog.element().querySelectorAll('.app-confirm-dialog__actions button')).toHaveLength(1)
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(name).toHaveValue('保留未保存组织')
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
    expect(onCommitted).not.toHaveBeenCalled()
    fail = false
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(instances).toHaveLength(1)
  })

  it('automatically merges a save version conflict and waits for the user to save the updated draft', async () => {
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
    await description.fill('保存前的本地说明')
    const original = service.request.getMockImplementation()!
    let conflict = true
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'saveInstance' && conflict) {
        conflict = false
        const updated = structuredClone(instances[0])
        updated.revision = 2
        updated.definition.nodes[1].name = '远端交付成员'
        updated.bindings[0].conversationId = 'chat-c'
        instances = [updated]
        throw Object.assign(
          new Error('Organization changed or was deleted; reload before changing it'),
          { code: -32009 }
        )
      }
      return original(input)
    })
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.element(page.getByRole('alertdialog')).toHaveTextContent('请检查后再次保存')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expect.element(description).toHaveValue('保存前的本地说明')
    await page.getByRole('tab', { name: '组织设计', exact: true }).click()
    await expect.element(agent('远端交付成员')).toBeVisible()
    await expect.element(agent('需求分析')).toHaveTextContent('自动化验收讨论')
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
    const saves = () =>
      service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    expect(saves()).toHaveLength(1)
    expect(onCommitted).not.toHaveBeenCalled()
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(saves()).toHaveLength(2)
    expect(saves()[1][0]).toMatchObject({
      expectedRevision: 2,
      definition: {
        description: '保存前的本地说明',
        nodes: [{ id: 'analysis' }, { id: 'delivery', name: '远端交付成员' }]
      },
      bindings: [
        { nodeId: 'analysis', conversationId: 'chat-c' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ]
    })
  })

  it('refreshes a changed template without losing compatible staged conversation assignments', async () => {
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    record = {
      ...record,
      revision: 2,
      definition: {
        ...record.definition,
        nodes: record.definition.nodes.filter((node) => node.id === 'analysis')
      }
    }
    window.dispatchEvent(new Event('captain:workflows-changed'))
    expect(page.getByRole('button', { name: '刷新版本', exact: true }).query()).toBeNull()
    await expect.poll(() => document.querySelectorAll('[data-workflow-node-id]').length).toBe(1)
    await expectBoundCount(1)
    await page.getByRole('button', { name: /^(激活|保存)$/, exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({
      expectedTemplateRevision: 2,
      bindings: [{ nodeId: 'analysis', conversationId: 'chat-a' }]
    })
  })

  it('edits a running organization and removes only the selected organization', async () => {
    instances = [
      {
        ...instance('live', '运行组织', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        running: true
      },
      {
        ...instance('idle', '可移除组织', [{ nodeId: 'analysis', conversationId: 'chat-b' }]),
        color: WORKFLOW_COLORS[1]
      }
    ]
    mount()
    await page.getByRole('button', { name: '配置 运行组织' }).click()
    await expect.element(page.getByRole('button', { name: '保存', exact: true })).toBeEnabled()
    expect(page.getByRole('button', { name: /^模版:/ }).query()).toBeNull()
    await dropConversation('需求分析', 'chat-c')
    await expect.element(agent('需求分析')).toHaveTextContent('自动化验收讨论')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    await page.getByRole('button', { name: '移除组织 可移除组织' }).click()
    await expect.element(page.getByRole('dialog')).toHaveTextContent('所有对话都会保留')
    await page.getByRole('button', { name: '移除', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(2)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'deleteInstance')?.[0]
    ).toEqual({ operation: 'deleteInstance', id: 'idle', expectedRevision: 1 })
  })

  it('selects existing conversations and clears only the selected assignment before saving', async () => {
    instances = [
      instance('existing', '已配置组织', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    mount()
    await page.getByRole('button', { name: '配置 已配置组织' }).click()
    expect(page.getByRole('button', { name: /^绑定对话 · 需求分析:/ }).query()).toBeNull()
    await userEvent.dblClick(agent('需求分析'))
    const binding = page.getByRole('button', { name: /^绑定对话 · 需求分析:/ })
    const task = page.getByRole('textbox', { name: '这个节点会收到什么', exact: true })
    expect(binding.element().closest('.workflow-graph-inspector')).not.toBeNull()
    expect(binding.element().getBoundingClientRect().bottom).toBeLessThan(
      task.element().getBoundingClientRect().top
    )
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/organization-binding-inspector.png'
    })
    await page.getByRole('button', { name: /^绑定对话 · 需求分析:/ }).click()
    await page.getByRole('option', { name: '自动化验收讨论', exact: true }).click()
    await expect.element(agent('需求分析')).toHaveTextContent('自动化验收讨论')
    await page.getByRole('button', { name: /^绑定对话 · 需求分析:/ }).click()
    await page.getByRole('option', { name: '保存时新建对话', exact: true }).click()
    await expectBoundCount(1)
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const saves = service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    expect(saves).toHaveLength(1)
    expect(saves[0][0]).toMatchObject({
      id: 'existing',
      expectedRevision: 1,
      bindings: [
        { nodeId: 'analysis', conversationId: null },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ]
    })
    expect(saves[0][0]).not.toHaveProperty('templateId')
    expect(saves[0][0]).not.toHaveProperty('expectedTemplateRevision')
  })

  it('explains incomplete member configuration before saving or creating conversations', async () => {
    const original = service.request.getMockImplementation()!
    let incomplete = true
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'validate' && incomplete)
        return { records: [], issues: [{ code: 'task', subject: 'analysis' }] }
      return original(input)
    })
    mount()
    await begin()
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect
      .element(page.getByRole('alertdialog', { name: '请完善组织配置', exact: true }))
      .toHaveTextContent('需求分析: 请填写节点名称和任务。')
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
    expect(onBeforeCommit).not.toHaveBeenCalled()
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    incomplete = false
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
  })
})
