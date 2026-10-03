import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { useCallback, useEffect, useState } from 'react'
import type {
  RightSidebarPage,
  RightSidebarPageUpdate
} from '../../features/rightSidebar/rightSidebarTypes'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowRequest,
  WorkflowResponse,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import type { ChatComposerDraft, ChatConversation } from '../../features/chat/chatTypes'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import { ToastProvider } from '../../components/toast/ToastProvider'
import {
  WORKFLOW_COLORS,
  WORKFLOW_CONVERSATION_DRAG_TYPE
} from '../../features/workflows/project/projectWorkflowText'
import '../../styles/global.css'

const service = vi.hoisted(() => ({
  request: vi.fn(),
  runningIds: null as Set<string> | null,
  runtimeListeners: new Set<(snapshot: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: vi.fn((listener: (snapshot: WorkflowRuntimeSnapshot) => void) => {
        service.runtimeListeners.add(listener)
        return () => service.runtimeListeners.delete(listener)
      })
    }
  }
}))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
vi.mock('../../features/workflows/project/useWorkflowActivity', () => ({
  useWorkflowActivity: (items: WorkflowInstance[]) => ({
    runningInstanceIds:
      service.runningIds ??
      new Set(items.filter((item) => item.enabled && item.running).map((item) => item.id)),
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
    resolvedColorScheme: 'dark',
    t: (key: string) => key
  })
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: null } })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
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

const { WorkflowsPage } = await import('../../features/workflows/project/WorkflowsPage')
const { WorkflowSidebarPage } = await import('../../features/workflows/project/WorkflowSidebarPage')

const conversation = (id: string, title: string, projectId: string): ChatConversation => ({
  id,
  title,
  projectId,
  modelId: 'chat-model',
  messages: [],
  createdAt: 1,
  updatedAt: 1
})
const conversations = [
  conversation('chat-a', '产品需求讨论', 'project-a'),
  conversation('chat-b', '界面设计讨论', 'project-b'),
  conversation('chat-c', '自动化验收讨论', 'project-b')
]
const projects = [
  { id: 'project-a', name: '产品项目', folders: [], createdAt: 1 },
  { id: 'project-b', name: '设计项目', folders: [], createdAt: 1 }
]
let record: WorkflowRecord
let extraRecords: WorkflowRecord[]
let instances: WorkflowInstance[]
let onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>()
let onBeforeCommit = vi.fn<(conversationIds: readonly string[]) => Promise<void>>()

const instance = (
  id: string,
  name: string,
  bindings: WorkflowInstance['bindings']
): WorkflowInstance => ({
  id,
  name,
  bindings,
  templateId: 'template-a',
  templateRevision: 1,
  definition: structuredClone(record.definition),
  revision: 1,
  updatedAt: 1,
  color: '#4F8FEA',
  enabled: true,
  running: false,
  needsReview: false
})

beforeEach(async () => {
  await page.viewport(1440, 900)
  const css = getFrontendCssVariables(undefined, classicDarkTheme)
  for (const [key, value] of Object.entries(css))
    document.documentElement.style.setProperty(key, value)
  record = {
    enabled: true,
    revision: 1,
    updatedAt: 1,
    issues: [],
    definition: {
      schemaVersion: 1,
      id: 'template-a',
      name: '产品交付流程',
      description: '需求分析、开发与验收',
      background: '',
      nodes: [
        {
          id: 'analysis',
          kind: 'agent',
          name: '需求分析',
          x: 300,
          y: 160,
          modelConfigId: 'node-model',
          permissionMode: 'custom',
          receives: '',
          task: '分析需求',
          delivers: ''
        },
        {
          id: 'delivery',
          kind: 'agent',
          name: '开发交付',
          x: 610,
          y: 160,
          modelConfigId: 'node-model',
          permissionMode: 'default',
          receives: '',
          task: '完成交付',
          delivers: ''
        }
      ],
      viewport: { x: 0, y: 0, zoom: 1 }
    }
  }
  instances = []
  service.runningIds = null
  service.runtimeListeners.clear()
  extraRecords = []
  onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>().mockResolvedValue(undefined)
  onBeforeCommit = vi
    .fn<(conversationIds: readonly string[]) => Promise<void>>()
    .mockResolvedValue(undefined)
  service.request
    .mockReset()
    .mockImplementation(async (input: WorkflowRequest): Promise<WorkflowResponse> => {
      if (input.operation === 'nodeMessages') {
        return {
          records: [],
          issues: [],
          nodeMessages: {
            instanceId: input.instanceId,
            nodeId: input.nodeId,
            messages: [],
            nextBeforeSequence: null
          }
        }
      }
      if (input.operation === 'saveInstance') {
        if (
          instances.some(
            (workflow) =>
              workflow.enabled &&
              workflow.id !== input.id &&
              workflow.color.toLowerCase() === input.color.toLowerCase()
          )
        )
          throw Object.assign(new Error('workflow_color_in_use'), { code: -32009 })
        const created = {
          ...instance(
            input.id,
            input.name,
            input.bindings.map((binding) => ({
              nodeId: binding.nodeId,
              conversationId: binding.conversationId ?? `created-${binding.nodeId}`
            }))
          ),
          enabled: instances.find((item) => item.id === input.id)?.enabled ?? true,
          definition: structuredClone(input.definition ?? record.definition),
          color: input.color,
          projectId: input.projectId ?? null
        }
        instances = [...instances.filter((item) => item.id !== input.id), created]
      }
      if (input.operation === 'deleteInstance')
        instances = instances.filter((item) => item.id !== input.id)
      if (input.operation === 'setInstanceEnabled') {
        const current = instances.find((item) => item.id === input.id)!
        if (
          input.enabled &&
          instances.some(
            (item) =>
              item.id !== input.id &&
              item.enabled &&
              item.color.toLowerCase() === current.color.toLowerCase()
          )
        )
          throw new Error('workflow_color_in_use')
        instances = instances.map((item) =>
          item.id === input.id
            ? { ...item, enabled: input.enabled, revision: item.revision + 1 }
            : item
        )
      }
      return {
        records: [record, ...extraRecords],
        instances,
        issues: [],
        affectedConversationIds: input.operation === 'saveInstance' ? ['chat-a'] : []
      }
    })
})

afterEach(() => vi.useRealTimers())

function renderPage(extra: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) {
  return (
    <ToastProvider>
      <div style={{ width: '100vw', height: '100vh' }}>
        <WorkflowsPage
          conversations={conversations}
          projects={projects}
          onManageTemplates={vi.fn()}
          onCommitted={onCommitted}
          onBeforeCommit={onBeforeCommit}
          {...extra}
        />
      </div>
    </ToastProvider>
  )
}

function mount(extra: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) {
  return render(renderPage(extra))
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function membersChanged(instanceId: string, sequence: number) {
  for (const listener of service.runtimeListeners)
    listener({
      instanceId,
      sequence,
      inputs: [],
      events: [
        {
          sequence,
          instanceId,
          inputId: null,
          messageId: null,
          sourceNodeId: null,
          targetNodeId: null,
          kind: 'members_changed',
          createdAt: sequence
        }
      ]
    })
}

async function begin() {
  await expect.element(page.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
  const activate = document.querySelector(
    '.project-workflows__header button.workflow-button--primary'
  ) as HTMLButtonElement
  await expect.poll(() => activate.disabled).toBe(false)
  activate.click()
  await chooseTemplate('产品交付流程')
}

async function chooseTemplate(name: string) {
  await page.getByRole('button', { name: /^模版:/ }).click()
  await page.getByRole('option', { name, exact: true }).click()
}

function agent(name: string) {
  return page.getByRole('group', { name: `节点 ${name}`, exact: true })
}

async function dropConversation(nodeName: string, conversationId: string) {
  const target = agent(nodeName)
  await expect.element(target).toBeVisible()
  const dataTransfer = new DataTransfer()
  dataTransfer.setData(WORKFLOW_CONVERSATION_DRAG_TYPE, conversationId)
  target
    .element()
    .dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer }))
  target
    .element()
    .dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }))
}

async function expectBoundCount(count: number) {
  await expect.poll(() => document.querySelectorAll('.workflow-node.is-bound').length).toBe(count)
}

function SidebarWorkflowHarness({
  instanceId = null,
  navigationId = 1,
  chats = conversations,
  onOpenConversation = () => undefined
}: {
  instanceId?: string | null
  navigationId?: number
  chats?: ChatConversation[]
  onOpenConversation?: (id: string) => void
}) {
  const [tab, setTab] = useState<RightSidebarPage>({
    id: 'workflow-tab',
    moduleId: 'workflows',
    title: '组织',
    moduleState: { kind: 'workflows', instanceId, navigationId }
  })
  useEffect(() => {
    setTab((current) => ({
      ...current,
      moduleState: { kind: 'workflows', instanceId, navigationId }
    }))
  }, [instanceId, navigationId])
  const onPageUpdate = useCallback((update: RightSidebarPageUpdate) => {
    setTab((current) => ({ ...current, ...update }))
  }, [])
  return (
    <ToastProvider>
      <div style={{ width: '100vw', height: '100vh' }}>
        <span data-testid="workflow-tab-title">{tab.title}</span>
        <WorkflowSidebarPage
          context={{
            page: tab,
            activity: 'foreground',
            availability: 'available',
            isSelected: true,
            onPageUpdate,
            onOpenPage: () => undefined,
            onSurfaceFocus: () => undefined,
            t: (key) => key
          }}
          conversations={chats}
          projects={projects}
          onCommitted={onCommitted}
          onManageTemplates={() => undefined}
          onOpenConversation={onOpenConversation}
        />
      </div>
    </ToastProvider>
  )
}

describe('organization sidebar adapter', () => {
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
        ['list', 'listInstances', 'nodeMessages'].includes(input.operation)
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
      service.request.mock.calls.filter(([input]) => input.operation === 'list').length
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
      path: '../../../../../.cache/workflow-authoring/global-workflow-empty-canvas-dark.png'
    })
    expect(document.querySelector('.project-workflows__template')).toBeNull()
    expect(agent('需求分析').query()).toBeNull()
    await page.getByRole('button', { name: /^模版:/ }).click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-template-menu-dark.png'
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

  it('switches organization availability without reinitializing conversations, including disabling a running organization', async () => {
    instances = [
      {
        ...instance('live', '运行组织', [
          { nodeId: 'analysis', conversationId: 'chat-a' },
          { nodeId: 'delivery', conversationId: 'chat-b' }
        ]),
        running: true
      }
    ]
    await mount()
    const disable = page.getByRole('switch', { name: '停用组织 运行组织', exact: true })
    await expect.element(disable).toBeEnabled()
    const configure = page.getByRole('button', { name: '配置 运行组织', exact: true })
    expect(disable.element().getBoundingClientRect().right).toBeLessThan(
      configure.element().getBoundingClientRect().left
    )
    await disable.click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const enable = page.getByRole('switch', { name: '启用组织 运行组织', exact: true })
    await expect.element(enable).toHaveAttribute('aria-checked', 'false')
    await enable.click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(2)
    expect(onBeforeCommit).not.toHaveBeenCalled()
    expect(
      service.request.mock.calls
        .filter(([request]) => request.operation === 'setInstanceEnabled')
        .map(([request]) => request)
    ).toEqual([
      { operation: 'setInstanceEnabled', id: 'live', enabled: false, expectedRevision: 1 },
      { operation: 'setInstanceEnabled', id: 'live', enabled: true, expectedRevision: 2 }
    ])
    expect(
      onCommitted.mock.calls.every(([response]) => response.affectedConversationIds?.length === 0)
    ).toBe(true)
  })

  it('animates only active organizations in their own color without moving card content', async () => {
    instances = [
      { ...instance('pink', '粉色组织', []), color: WORKFLOW_COLORS[4] },
      { ...instance('green', '绿色组织', []), color: WORKFLOW_COLORS[2] },
      { ...instance('off', '停用组织', []), enabled: false }
    ]
    service.runningIds = new Set()
    const screen = await mount()
    await expect
      .element(page.getByRole('button', { name: '配置 粉色组织', exact: true }))
      .toBeVisible()
    const cards = Array.from(document.querySelectorAll<HTMLElement>('.project-workflows__instance'))
    const positions = () =>
      cards.map((card) =>
        Array.from(card.querySelectorAll('button')).map((button) =>
          button.getBoundingClientRect().toJSON()
        )
      )
    const before = positions()
    expect(cards.every((card) => card.getAnimations({ subtree: true }).length === 0)).toBe(true)
    service.runningIds = new Set(['pink', 'green', 'off'])
    await screen.rerender(renderPage())
    expect(cards.map((card) => card.classList.contains('is-running'))).toEqual([true, true, false])
    expect(positions()).toEqual(before)
    for (const [index, color] of [WORKFLOW_COLORS[4], WORKFLOW_COLORS[2]].entries()) {
      const light = cards[index].querySelector<SVGRectElement>('.project-workflows__activity-head')!
      expect(cards[index].style.getPropertyValue('--workflow-color')).toBe(color)
      expect(getComputedStyle(light).animationName).toBe('workflow-activity-orbit')
      expect(getComputedStyle(light).visibility).toBe('visible')
      expect(light.getBBox().width).toBeGreaterThan(cards[index].clientWidth - 5)
      expect(
        getComputedStyle(cards[index].querySelector('.project-workflows__activity')!).pointerEvents
      ).toBe('none')
    }
    for (const card of cards)
      for (const animation of card.getAnimations({ subtree: true })) {
        animation.pause()
        animation.currentTime = 1500
      }
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-running-lights-dark.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-running-lights-light.png'
    })
    service.runningIds = new Set()
    await screen.rerender(renderPage())
    expect(cards.every((card) => !card.classList.contains('is-running'))).toBe(true)
    expect(cards.every((card) => card.getAnimations({ subtree: true }).length === 0)).toBe(true)
    expect(positions()).toEqual(before)
  })

  it('places the durable activity duration after the conversation count and omits it before first activity', async () => {
    instances = [
      {
        ...instance('timed', '持续协作', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        activity: { startedAt: 1_000, completedAt: 94_028_000 }
      },
      instance('new', '尚未运行', [])
    ]
    await mount()
    await expect.element(page.getByText('已连续运行 1d 2h 7m 7s', { exact: true })).toBeVisible()
    const elapsed = document.querySelector<HTMLElement>('.project-workflows__elapsed')!
    const metadata = elapsed.closest('.project-workflows__instance-meta')!
    expect(metadata.textContent).toBe('1 个对话已连续运行 1d 2h 7m 7s')
    expect(metadata.firstElementChild!.getBoundingClientRect().right).toBeLessThan(
      elapsed.getBoundingClientRect().left
    )
    expect(document.querySelectorAll('.project-workflows__elapsed')).toHaveLength(1)
  })

  it('keeps every card component visually unchanged while an organization switch is pending', async () => {
    instances = [
      instance('first', '切换流程', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ]),
      { ...instance('second', '另一流程', []), color: WORKFLOW_COLORS[1] },
      ...Array.from({ length: 16 }, (_, index) => ({
        ...instance(`inactive-${index}`, `停用流程${index}`, []),
        enabled: false
      }))
    ]
    await mount()
    const toggle = page.getByRole('switch', { name: '停用组织 切换流程', exact: true })
    await expect.element(toggle).toBeVisible()
    const library = document.querySelector('.project-workflows__library') as HTMLElement
    const cards = Array.from(library.querySelectorAll('.project-workflows__instance'))
    const names = cards.map((card) => card.querySelector('strong')?.textContent)
    const otherConfigure = page.getByRole('button', { name: '配置 另一流程', exact: true })
    const manage = page.getByRole('button', { name: '管理组织模板', exact: true })
    const gate = deferred()
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation !== 'setInstanceEnabled') return original(input)
      await gate.promise
      const response = await original(input)
      window.dispatchEvent(new Event('captain:workflows-changed'))
      return { ...response, instances: [...response.instances!].reverse() }
    })
    const listRequests = () =>
      service.request.mock.calls.filter(([input]) =>
        ['list', 'listInstances', 'nodeMessages'].includes(input.operation)
      ).length
    const initialReads = listRequests()
    library.scrollTop = 160
    const card = cards[0]
    const toggleElement = toggle.element()
    const visualState = () =>
      Array.from(card.querySelectorAll('button, strong, small, svg')).map((element) => {
        const style = getComputedStyle(element)
        const rect = element.getBoundingClientRect()
        return {
          element,
          opacity: style.opacity,
          color: style.color,
          background: element === toggleElement ? undefined : style.backgroundColor,
          x: rect.x,
          y: rect.y,
          width: rect.width,
          height: rect.height
        }
      })
    const before = visualState()
    ;(toggle.element() as HTMLButtonElement).click()
    await expect.element(toggle).toHaveAttribute('aria-busy', 'true')
    expect(visualState()).toEqual(before)
    // Busy controls keep their appearance and focus, while handlers reject repeat actions.
    ;(toggle.element() as HTMLButtonElement).click()
    ;(card.querySelector('.project-workflows__configure') as HTMLButtonElement).click()
    ;(
      page
        .getByRole('button', { name: '移除组织 切换流程', exact: true })
        .element() as HTMLButtonElement
    ).click()
    expect(page.getByRole('dialog').query()).toBeNull()
    expect(document.querySelector('.project-workflows__binding-toolbar')).toBeNull()
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'setInstanceEnabled')
    ).toHaveLength(1)
    window.dispatchEvent(new Event('captain:workflows-changed'))
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(Array.from(library.querySelectorAll('.project-workflows__instance'))).toEqual(cards)
    expect(library.scrollTop).toBe(160)
    await expect.element(otherConfigure).toBeEnabled()
    await expect.element(manage).toBeEnabled()
    expect(getComputedStyle(otherConfigure.element()).opacity).toBe('1')
    expect(getComputedStyle(manage.element()).opacity).toBe('1')
    expect(page.getByText('正在加载组织…', { exact: true }).query()).toBeNull()
    expect(listRequests()).toBe(initialReads)
    gate.resolve()
    await expect
      .element(page.getByRole('switch', { name: '启用组织 切换流程', exact: true }))
      .toHaveAttribute('aria-checked', 'false')
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(Array.from(library.querySelectorAll('.project-workflows__instance'))).toEqual(cards)
    expect(Array.from(library.querySelectorAll('strong')).map((card) => card.textContent)).toEqual(
      names
    )
    expect(library.scrollTop).toBe(160)
    expect(visualState()).toEqual(before)
    // The deferred notification triggers one read after unlock without remounting any card.
    expect(listRequests()).toBe(initialReads + 1)
    expect(onCommitted).toHaveBeenCalledTimes(1)
  })

  it('keeps the list mounted during background refresh and ignores an old snapshot after a switch', async () => {
    instances = [
      instance('live', '后台刷新流程', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    await mount()
    const toggle = page.getByRole('switch', { name: '停用组织 后台刷新流程', exact: true })
    await expect.element(toggle).toBeVisible()
    const library = document.querySelector('.project-workflows__library')
    const card = document.querySelector('.project-workflows__instance')
    const gate = deferred()
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      const response = await original(input)
      if (input.operation === 'list') {
        await gate.promise
      }
      return response
    })
    const initialCalls = service.request.mock.calls.length
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.poll(() => service.request.mock.calls.length).toBe(initialCalls + 1)
    await expect.element(toggle).toBeVisible()
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(document.querySelector('.project-workflows__instance')).toBe(card)
    expect(page.getByText('正在加载组织…', { exact: true }).query()).toBeNull()
    await toggle.click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    gate.resolve()
    await expect
      .element(page.getByRole('switch', { name: '启用组织 后台刷新流程', exact: true }))
      .toHaveAttribute('aria-checked', 'false')
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(document.querySelector('.project-workflows__instance')).toBe(card)
    await expect
      .element(page.getByRole('button', { name: '激活新组织', exact: true }))
      .toBeEnabled()
  })

  it('preserves another organization draft opened while a switch is pending', async () => {
    instances = [
      instance('first', '切换流程', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ]),
      {
        ...instance('second', '配置流程', [{ nodeId: 'analysis', conversationId: 'chat-b' }]),
        color: WORKFLOW_COLORS[1]
      }
    ]
    await mount()
    const gate = deferred()
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation === 'setInstanceEnabled') await gate.promise
      return original(input)
    })
    await page.getByRole('switch', { name: '停用组织 切换流程', exact: true }).click()
    await page.getByRole('button', { name: '配置 配置流程', exact: true }).click()
    const name = page.getByRole('textbox', { name: '名称', exact: true })
    await name.fill('已编辑的配置流程')
    const canvas = document.querySelector('.workflow-canvas-shell')
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeDisabled()
    gate.resolve()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    await expect.element(name).toHaveValue('已编辑的配置流程')
    expect(document.querySelector('.workflow-canvas-shell')).toBe(canvas)
    await expect.element(agent('需求分析')).toHaveTextContent('界面设计讨论')
    await expect
      .element(page.getByRole('button', { name: /^(激活|保存)$/, exact: true }))
      .toBeEnabled()
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

  it('shows a failed load separately from an empty library and retries with a fresh snapshot', async () => {
    service.request.mockRejectedValueOnce(
      Object.assign(new Error('Organization storage is unavailable'), {
        code: -32000,
        data: { privateGraphData: 'do-not-display-payload' }
      })
    )
    mount()
    const dialog = page.getByRole('alertdialog', { name: '暂时无法完成操作' })
    await expect.element(dialog).toHaveTextContent('组织加载失败')
    expect(dialog.element().querySelectorAll('.app-confirm-dialog__actions button')).toHaveLength(1)
    expect(document.body.textContent).not.toContain('Organization storage is unavailable')
    expect(document.body.textContent).not.toContain('do-not-display-payload')
    expect(page.getByText('错误详情', { exact: true }).query()).toBeNull()
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    expect(dialog.query()).toBeNull()
    await expect.element(page.getByText('暂时无法读取组织')).toBeVisible()
    await expect
      .element(page.getByRole('button', { name: '激活新组织', exact: true }))
      .toBeDisabled()
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await expect.element(page.getByRole('heading', { name: '让对话一起协作' })).toBeVisible()
    expect(document.querySelector('.project-workflows__error-detail')).toBeNull()
    expect(service.request.mock.calls.filter(([input]) => input.operation === 'list')).toHaveLength(
      2
    )
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'listInstances')
    ).toHaveLength(0)
    await begin()
    await expect.element(page.getByRole('textbox', { name: '名称', exact: true })).toBeVisible()
  })

  it('defers background errors until the organization tab is foreground and does not reopen acknowledged errors', async () => {
    service.request.mockRejectedValueOnce(new Error('Storage unavailable'))
    const view = await mount({ foreground: false })
    await expect.element(page.getByText('暂时无法读取组织')).toBeVisible()
    expect(page.getByRole('alertdialog').query()).toBeNull()
    await view.rerender(renderPage({ foreground: true }))
    await expect.element(page.getByRole('alertdialog')).toHaveTextContent('组织加载失败')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await view.rerender(renderPage({ foreground: false }))
    await view.rerender(renderPage({ foreground: true }))
    expect(page.getByRole('alertdialog').query()).toBeNull()
    await expect.element(page.getByRole('button', { name: '重试', exact: true })).toBeVisible()
  })

  it('opens template management from the library and empty activation canvas', async () => {
    const onManageTemplates = vi.fn()
    const first = await mount({ onManageTemplates })
    await page.getByRole('button', { name: '管理组织模板', exact: true }).click()
    expect(onManageTemplates).toHaveBeenCalledTimes(1)
    expect(page.getByRole('button', { name: '新建模板', exact: true }).query()).toBeNull()
    await first.unmount()
    service.request.mockResolvedValue({ records: [], instances: [], issues: [] })
    await mount({ onManageTemplates })
    await page.getByRole('button', { name: '激活新组织', exact: true }).first().click()
    await expect.element(page.getByText('还没有可用的组织模板', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '管理组织模板', exact: true }).last().click()
    expect(onManageTemplates).toHaveBeenCalledTimes(2)
    expect(page.getByRole('button', { name: '新建模板', exact: true }).query()).toBeNull()
    expect(
      service.request.mock.calls.every(([request]) =>
        ['list', 'listInstances'].includes(request.operation)
      )
    ).toBe(true)
  })

  it('shows compact organization cards with settings actions and no redundant status badges', async () => {
    instances = [
      instance('normal', '需求协作', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
      {
        ...instance('review', '需确认流程', []),
        color: WORKFLOW_COLORS[1],
        needsReview: true,
        enabled: false
      },
      { ...instance('running', '执行中流程', []), color: WORKFLOW_COLORS[2], running: true }
    ]
    await mount()
    await expect
      .element(page.getByRole('button', { name: '配置 需求协作', exact: true }))
      .toBeVisible()
    expect(page.getByText('已配置', { exact: true }).query()).toBeNull()
    expect(page.getByText('需要重新确认', { exact: true }).query()).toBeNull()
    expect(page.getByText('运行中', { exact: true }).query()).toBeNull()
    const cards = Array.from(document.querySelectorAll('.project-workflows__instance'))
    expect(cards).toHaveLength(3)
    for (const card of cards) {
      expect(card.getBoundingClientRect().height).toBeLessThanOrEqual(76)
      expect(card.querySelector('.lucide-chevron-right')).toBeNull()
    }
    const configure = page.getByRole('button', { name: '配置 需求协作', exact: true }).element()
    expect(configure.textContent).toBe('')
    expect(getComputedStyle(configure).borderTopColor).toBe('rgba(0, 0, 0, 0)')
    expect(getComputedStyle(configure).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    expect(configure.querySelector('svg')).not.toBeNull()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-cards-compact-dark.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-cards-compact-light.png'
    })
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
      path: '../../../../../.cache/workflow-authoring/workflow-project-toolbar-dark.png'
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
      path: '../../../../../.cache/workflow-authoring/global-workflow-assignment-conflict.png'
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
      path: '../../../../../.cache/workflow-authoring/organization-home-save-error-dialog.png'
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
      path: '../../../../../.cache/workflow-authoring/organization-binding-inspector.png'
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

  it('collapses organization colors into a single-row palette that closes on selection, outside click and Escape', async () => {
    mount()
    await begin()
    const palette = page.getByRole('button', { name: '组织颜色', exact: true })
    const popover = page.getByRole('dialog', { name: '组织颜色', exact: true })
    expect(page.getByRole('button', { name: '标记颜色 2', exact: true }).query()).toBeNull()
    await palette.click()
    await expect.element(popover).toBeVisible()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-palette-dark.png'
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
    await page.screenshot({ path: '../../../../../.cache/organizations/live-editor.png' })
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
    await page.screenshot({ path: '../../../../../.cache/organizations/context-editor.png' })
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
      path: '../../../../../.cache/workflow-authoring/organization-automatic-update.png'
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
