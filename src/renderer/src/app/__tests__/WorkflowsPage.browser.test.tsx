import { page, userEvent } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowRequest,
  WorkflowResponse
} from '@mycopilot/protocol'
import type { ChatComposerDraft, ChatConversation } from '../../features/chat/chatTypes'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import {
  WORKFLOW_COLORS,
  WORKFLOW_CONVERSATION_DRAG_TYPE
} from '../../features/workflows/project/projectWorkflowText'
import '../../styles/global.css'

const service = vi.hoisted(() => ({ request: vi.fn(), runningIds: null as Set<string> | null }))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
vi.mock('../../features/workflows/project/useWorkflowActivity', () => ({
  useWorkflowActivity: (items: WorkflowInstance[]) =>
    service.runningIds ??
    new Set(items.filter((item) => item.enabled && item.running).map((item) => item.id))
}))
vi.mock('../../features/workflows/project/useWorkflowMonitor', () => ({
  useWorkflowMonitor: () => ({
    runningConversationIds: new Set<string>(),
    waitingApprovalConversationIds: new Set<string>()
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', resolvedColorScheme: 'dark' })
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: null } })
}))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    models: [
      { id: 'node-model', displayName: '节点默认模型', execution: { status: 'available' } },
      { id: 'chat-model', displayName: '已有对话模型', execution: { status: 'available' } },
      { id: 'user-model', displayName: '对话后续模型', execution: { status: 'available' } }
    ]
  })
}))

const { WorkflowsPage } = await import('../../features/workflows/project/WorkflowsPage')

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
      flows: [
        {
          id: 'flow-1',
          name: 'S1',
          source: { kind: 'boundary' },
          target: { kind: 'node', nodeId: 'analysis' }
        },
        {
          id: 'flow-2',
          name: 'S2',
          source: { kind: 'node', nodeId: 'analysis' },
          target: { kind: 'node', nodeId: 'delivery' }
        }
      ],
      viewport: { x: 0, y: 0, zoom: 1 },
      boundaryPositions: { input: { x: 20, y: 160 } }
    }
  }
  instances = []
  service.runningIds = null
  extraRecords = []
  onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>().mockResolvedValue(undefined)
  onBeforeCommit = vi
    .fn<(conversationIds: readonly string[]) => Promise<void>>()
    .mockResolvedValue(undefined)
  service.request
    .mockReset()
    .mockImplementation(async (input: WorkflowRequest): Promise<WorkflowResponse> => {
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
          color: input.color
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

async function begin() {
  await expect.element(page.getByRole('heading', { name: '工作流', exact: true })).toBeVisible()
  const activate = document.querySelector(
    '.project-workflows__header button.workflow-button--primary'
  ) as HTMLButtonElement
  await expect.poll(() => activate.disabled).toBe(false)
  activate.click()
  await chooseTemplate('产品交付流程')
}

async function chooseTemplate(name: string) {
  await page.getByRole('button', { name: /^工作流模板:/ }).click()
  await page.getByRole('option', { name, exact: true }).click()
}

function agent(name: string) {
  return page.getByRole('button', { name: `绑定对话 · ${name}`, exact: true })
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
  await expect
    .poll(() => document.querySelectorAll('.workflow-binding-node.is-bound').length)
    .toBe(count)
}

async function nodeDetails(nodeName: string) {
  await agent(nodeName).hover()
  const tooltip = page.getByRole('tooltip')
  await expect.element(tooltip).toBeVisible()
  return tooltip
}

describe('global workflow management', () => {
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
    const diagram = page.getByRole('button', { name: '查看流程图 交付看板', exact: true })
    await expect.element(diagram).toBeVisible()
    const configure = page.getByRole('button', { name: '配置 交付看板', exact: true }).element()
    expect(
      diagram.element().compareDocumentPosition(configure) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy()
    await diagram.click()
    await expect
      .element(page.getByRole('button', { name: '返回工作流', exact: true }))
      .toBeVisible()
    expect(document.querySelector('.project-workflows__library')).toBeNull()
    expect(document.querySelector('.workflow-binding-node')).toBeNull()
    await expect.element(page.getByText('等待交互', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '打开对话 · 产品需求讨论', exact: true }).click()
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-a')
    expect(onMonitorChange).toHaveBeenCalledWith('workflow-a')
    expect(
      service.request.mock.calls.every(([input]) =>
        ['list', 'listInstances'].includes(input.operation)
      )
    ).toBe(true)
    await page.getByRole('button', { name: '返回工作流', exact: true }).click()
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
      .element(page.getByRole('button', { name: '打开对话 · 产品需求讨论', exact: true }))
      .toBeVisible()
    await view.rerender(renderPage({ initialMonitorId: 'workflow-b', onOpenConversation }))
    await page.getByRole('button', { name: '打开对话 · 界面设计讨论', exact: true }).click()
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-b')
    expect(document.querySelector('.project-workflows__library')).toBeNull()
    await view.rerender(renderPage({ initialMonitorId: null, onOpenConversation }))
    await expect
      .element(page.getByRole('button', { name: '查看流程图 停用看板', exact: true }))
      .toBeVisible()
  })

  it('keeps a missing diagram recoverable instead of showing another workflow', async () => {
    await mount({ initialMonitorId: 'missing' })
    await expect.element(page.getByText('工作流或模板已不可用', { exact: true })).toBeVisible()
    await expect.element(page.getByRole('button', { name: '重试', exact: true })).toBeVisible()
    await page.getByRole('button', { name: '返回工作流', exact: true }).click()
    await expect.element(page.getByRole('heading', { name: '工作流', exact: true })).toBeVisible()
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
    await page.getByRole('button', { name: '激活新工作流', exact: true }).first().click()
    await expect.element(page.getByLabelText('空白工作流画布')).toBeVisible()
    await expect.element(page.getByRole('button', { name: '激活', exact: true })).toBeDisabled()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-empty-canvas-dark.png'
    })
    expect(document.querySelector('.project-workflows__template')).toBeNull()
    expect(
      page.getByRole('button', { name: '绑定对话 · 需求分析', exact: true }).query()
    ).toBeNull()
    await page.getByRole('button', { name: /^工作流模板:/ }).click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-template-menu-dark.png'
    })
    expect(page.getByRole('option', { name: '未就绪模板', exact: true }).query()).toBeNull()
    await page.getByRole('option', { name: '产品交付流程', exact: true }).click()
    await expect.element(agent('需求分析')).toBeVisible()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('产品交付流程')
    await expect.element(page.getByRole('button', { name: '激活', exact: true })).toBeEnabled()
    const select = page
      .getByRole('button', { name: /^工作流模板:/ })
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
          nodes: record.definition.nodes.map((node) => ({ ...node, id: `new-${node.id}` })),
          flows: []
        }
      }
    ]
    await mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    await chooseTemplate('另一套流程')
    await expect.element(page.getByRole('dialog')).toHaveTextContent('放弃未保存的绑定')
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
      instance('existing', '已配置工作流', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await page.getByRole('button', { name: '配置 已配置工作流', exact: true }).click()
    await expect.element(page.getByRole('button', { name: /^工作流模板:/ })).toBeDisabled()
  })

  it('switches workflow availability without reinitializing conversations, including disabling a running workflow', async () => {
    instances = [
      {
        ...instance('live', '运行工作流', [
          { nodeId: 'analysis', conversationId: 'chat-a' },
          { nodeId: 'delivery', conversationId: 'chat-b' }
        ]),
        running: true
      }
    ]
    await mount()
    const disable = page.getByRole('switch', { name: '停用工作流 运行工作流', exact: true })
    await expect.element(disable).toBeEnabled()
    const configure = page.getByRole('button', { name: '配置 运行工作流', exact: true })
    expect(disable.element().getBoundingClientRect().right).toBeLessThan(
      configure.element().getBoundingClientRect().left
    )
    await disable.click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    const enable = page.getByRole('switch', { name: '启用工作流 运行工作流', exact: true })
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

  it('animates only active workflows in their own color without moving card content', async () => {
    instances = [
      { ...instance('pink', '粉色工作流', []), color: WORKFLOW_COLORS[4] },
      { ...instance('green', '绿色工作流', []), color: WORKFLOW_COLORS[2] },
      { ...instance('off', '停用工作流', []), enabled: false }
    ]
    service.runningIds = new Set()
    const screen = await mount()
    await expect
      .element(page.getByRole('button', { name: '配置 粉色工作流', exact: true }))
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

  it('keeps every card component visually unchanged while a workflow switch is pending', async () => {
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
    const toggle = page.getByRole('switch', { name: '停用工作流 切换流程', exact: true })
    await expect.element(toggle).toBeVisible()
    const library = document.querySelector('.project-workflows__library') as HTMLElement
    const cards = Array.from(library.querySelectorAll('.project-workflows__instance'))
    const names = cards.map((card) => card.querySelector('strong')?.textContent)
    const otherConfigure = page.getByRole('button', { name: '配置 另一流程', exact: true })
    const manage = page.getByRole('button', { name: '管理工作流模板', exact: true })
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
        ['list', 'listInstances'].includes(input.operation)
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
        .getByRole('button', { name: '移除工作流 切换流程', exact: true })
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
    expect(page.getByText('正在加载工作流…', { exact: true }).query()).toBeNull()
    expect(listRequests()).toBe(initialReads)
    gate.resolve()
    await expect
      .element(page.getByRole('switch', { name: '启用工作流 切换流程', exact: true }))
      .toHaveAttribute('aria-checked', 'false')
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(Array.from(library.querySelectorAll('.project-workflows__instance'))).toEqual(cards)
    expect(Array.from(library.querySelectorAll('strong')).map((card) => card.textContent)).toEqual(
      names
    )
    expect(library.scrollTop).toBe(160)
    expect(visualState()).toEqual(before)
    expect(listRequests()).toBe(initialReads)
    expect(onCommitted).toHaveBeenCalledTimes(1)
  })

  it('keeps the list mounted during background refresh and ignores an old snapshot after a switch', async () => {
    instances = [
      instance('live', '后台刷新流程', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
    ]
    await mount()
    const toggle = page.getByRole('switch', { name: '停用工作流 后台刷新流程', exact: true })
    await expect.element(toggle).toBeVisible()
    const library = document.querySelector('.project-workflows__library')
    const card = document.querySelector('.project-workflows__instance')
    const gate = deferred()
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      const response = await original(input)
      if (input.operation === 'list' || input.operation === 'listInstances') {
        await gate.promise
      }
      return response
    })
    const initialCalls = service.request.mock.calls.length
    window.dispatchEvent(new Event('captain:workflows-changed'))
    await expect.poll(() => service.request.mock.calls.length).toBe(initialCalls + 2)
    await expect.element(toggle).toBeVisible()
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(document.querySelector('.project-workflows__instance')).toBe(card)
    expect(page.getByText('正在加载工作流…', { exact: true }).query()).toBeNull()
    await toggle.click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    gate.resolve()
    await expect
      .element(page.getByRole('switch', { name: '启用工作流 后台刷新流程', exact: true }))
      .toHaveAttribute('aria-checked', 'false')
    expect(document.querySelector('.project-workflows__library')).toBe(library)
    expect(document.querySelector('.project-workflows__instance')).toBe(card)
    await expect
      .element(page.getByRole('button', { name: '激活新工作流', exact: true }))
      .toBeEnabled()
  })

  it('preserves another workflow draft opened while a switch is pending', async () => {
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
    await page.getByRole('switch', { name: '停用工作流 切换流程', exact: true }).click()
    await page.getByRole('button', { name: '配置 配置流程', exact: true }).click()
    const name = page.getByRole('textbox', { name: '名称', exact: true })
    await name.fill('已编辑的配置流程')
    const canvas = document.querySelector('.workflow-binding-canvas-shell')
    await expect.element(page.getByRole('button', { name: '激活', exact: true })).toBeDisabled()
    gate.resolve()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    await expect.element(name).toHaveValue('已编辑的配置流程')
    expect(document.querySelector('.workflow-binding-canvas-shell')).toBe(canvas)
    await expect.element(agent('需求分析')).toHaveTextContent('界面设计讨论')
    await expect.element(page.getByRole('button', { name: '激活', exact: true })).toBeEnabled()
  })

  it('reactivates a disabled workflow when its configuration is confirmed', async () => {
    instances = [
      {
        ...instance('paused', '已停用工作流', [
          { nodeId: 'analysis', conversationId: 'chat-a' },
          { nodeId: 'delivery', conversationId: 'chat-b' }
        ]),
        enabled: false
      }
    ]
    await mount()
    await page.getByRole('button', { name: '配置 已停用工作流', exact: true }).click()
    await expect
      .element(page.getByRole('button', { name: '工作流模板: 产品交付流程', exact: true }))
      .toBeDisabled()
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      onCommitted.mock.calls[0][0].instances?.find((item) => item.id === 'paused')?.enabled
    ).toBe(true)
    await expect
      .element(page.getByRole('switch', { name: '停用工作流 已停用工作流', exact: true }))
      .toHaveAttribute('aria-checked', 'true')
  })

  it('reuses disabled workflow colors but blocks enabling incomplete or conflicting workflows', async () => {
    instances = [
      { ...instance('disabled', '已停用工作流', []), enabled: false },
      {
        ...instance('invalid', '需重新确认工作流', []),
        enabled: false,
        needsReview: true,
        color: WORKFLOW_COLORS[1]
      }
    ]
    await mount()
    await expect
      .element(page.getByRole('switch', { name: '启用工作流 已停用工作流', exact: true }))
      .toBeDisabled()
    await begin()
    await page.getByRole('button', { name: '工作流颜色', exact: true }).click()
    await expect
      .element(
        page
          .getByRole('dialog', { name: '工作流颜色', exact: true })
          .getByRole('button', { name: '标记颜色 1', exact: true })
      )
      .toBeEnabled()
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([request]) => request.operation === 'saveInstance')?.[0]
    ).toMatchObject({ color: WORKFLOW_COLORS[0] })
  })

  it('shows a failed load separately from an empty library and retries with a fresh snapshot', async () => {
    service.request.mockRejectedValueOnce(
      Object.assign(new Error('Workflow storage is unavailable'), {
        code: -32000,
        data: { privateGraphData: 'do-not-display-payload' }
      })
    )
    mount()
    await expect.element(page.getByRole('alert')).toHaveTextContent('工作流加载失败')
    await page.getByText('错误详情', { exact: true }).click()
    await expect
      .element(page.getByRole('alert'))
      .toHaveTextContent('[-32000] Workflow storage is unavailable')
    expect(document.querySelector('[role="alert"]')?.textContent).not.toContain(
      'do-not-display-payload'
    )
    await expect.element(page.getByText('暂时无法读取工作流')).toBeVisible()
    await expect
      .element(page.getByRole('button', { name: '激活新工作流', exact: true }))
      .toBeDisabled()
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await expect.element(page.getByRole('heading', { name: '让对话一起协作' })).toBeVisible()
    expect(document.querySelector('.project-workflows__error-detail')).toBeNull()
    expect(service.request.mock.calls.filter(([input]) => input.operation === 'list')).toHaveLength(
      2
    )
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'listInstances')
    ).toHaveLength(2)
    await begin()
    await expect.element(page.getByRole('textbox', { name: '名称', exact: true })).toBeVisible()
  })

  it('opens template management from the library and empty activation canvas', async () => {
    const onManageTemplates = vi.fn()
    const first = await mount({ onManageTemplates })
    await page.getByRole('button', { name: '管理工作流模板', exact: true }).click()
    expect(onManageTemplates).toHaveBeenCalledTimes(1)
    expect(page.getByRole('button', { name: '新建模板', exact: true }).query()).toBeNull()
    await first.unmount()
    service.request.mockResolvedValue({ records: [], instances: [], issues: [] })
    await mount({ onManageTemplates })
    await page.getByRole('button', { name: '激活新工作流', exact: true }).first().click()
    await expect.element(page.getByText('还没有可用的工作流模板', { exact: true })).toBeVisible()
    await page.getByRole('button', { name: '管理工作流模板', exact: true }).last().click()
    expect(onManageTemplates).toHaveBeenCalledTimes(2)
    expect(page.getByRole('button', { name: '新建模板', exact: true }).query()).toBeNull()
    expect(
      service.request.mock.calls.every(([request]) =>
        ['list', 'listInstances'].includes(request.operation)
      )
    ).toBe(true)
  })

  it('shows compact workflow cards with explicit settings actions and only meaningful status badges', async () => {
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
    await expect.element(page.getByText('需要重新确认', { exact: true })).toBeVisible()
    await expect.element(page.getByText('运行中', { exact: true })).toBeVisible()
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

  it('retains its own color while disabling colors used by other workflows case-insensitively', async () => {
    instances = [
      {
        ...instance('own', '已有配色', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        color: '#4f8fea'
      },
      { ...instance('other', '另一种配色', []), color: '#b57bE8' }
    ]
    await mount()
    await page.getByRole('button', { name: '配置 已有配色', exact: true }).click()
    await page.getByRole('button', { name: '工作流颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '工作流颜色', exact: true })
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
    await page.getByRole('button', { name: '激活', exact: true }).click()
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

  it('chooses an unused default color when creating a workflow', async () => {
    instances = [{ ...instance('other', '已使用蓝色', []), color: '#4f8fea' }]
    await mount()
    await begin()
    await page.getByRole('button', { name: '工作流颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '工作流颜色', exact: true })
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 1', exact: true }))
      .toBeDisabled()
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 2', exact: true }))
      .toHaveAttribute('aria-pressed', 'true')
    await userEvent.keyboard('{Escape}')
    await page.getByRole('button', { name: '激活', exact: true }).click()
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
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect
      .element(page.getByRole('alert'))
      .toHaveTextContent('这个颜色已被其他工作流使用，请选择其他颜色。')
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    expect(instances).toHaveLength(1)
    expect(onCommitted).not.toHaveBeenCalled()
    expect(
      service.request.mock.calls.filter(([request]) => request.operation === 'saveInstance')
    ).toHaveLength(1)
    await page.getByRole('button', { name: '刷新版本', exact: true }).click()
    await expect.element(page.getByRole('status')).toHaveTextContent('已加载最新版本')
    const confirm = page
      .getByRole('button', { name: '激活', exact: true })
      .element() as HTMLButtonElement
    confirm.click()
    expect(
      service.request.mock.calls.filter(([request]) => request.operation === 'saveInstance')
    ).toHaveLength(1)
    await page.getByRole('button', { name: '工作流颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '工作流颜色', exact: true })
    await expect
      .element(palette.getByRole('button', { name: '标记颜色 1', exact: true }))
      .toBeDisabled()
    await palette.getByRole('button', { name: '标记颜色 2', exact: true }).click()
    await page.getByRole('button', { name: '激活', exact: true }).click()
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

  it('blocks new activation with a clear notice when all colors are used while keeping existing workflows configurable', async () => {
    instances = WORKFLOW_COLORS.map((color, index) => ({
      ...instance(`occupied-${index}`, `颜色流程${index + 1}`, []),
      color: color.toLowerCase()
    }))
    await mount()
    await expect
      .element(page.getByRole('button', { name: '配置 颜色流程1', exact: true }))
      .toBeVisible()
    await page.getByRole('button', { name: '激活新工作流', exact: true }).first().click()
    await expect
      .element(page.getByRole('alert'))
      .toHaveTextContent('可选颜色已全部被占用，请先停用或调整其他工作流。')
    expect(page.getByRole('textbox', { name: '名称', exact: true }).query()).toBeNull()
    expect(document.querySelector('.project-workflows__template')).toBeNull()
    expect(
      service.request.mock.calls.some(([request]) => request.operation === 'saveInstance')
    ).toBe(false)
    await page.getByRole('button', { name: '配置 颜色流程1', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('颜色流程1')
    await page.getByRole('button', { name: '工作流颜色', exact: true }).click()
    const palette = page.getByRole('dialog', { name: '工作流颜色', exact: true })
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
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('界面设计讨论')
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
    await expect.element(page.getByRole('heading', { name: '工作流', exact: true })).toBeVisible()
    expect(onBeforeCommit).not.toHaveBeenCalled()
    expect(onCommitted).not.toHaveBeenCalled()
  })

  it('previews node defaults only for changed bindings and retains later conversation model selections for unchanged bindings', async () => {
    instances = [
      instance('existing', '已配置工作流', [{ nodeId: 'analysis', conversationId: 'chat-a' }])
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
    await page.getByRole('button', { name: '配置 已配置工作流' }).click()
    await expect.element(await nodeDetails('需求分析')).toHaveTextContent('对话后续模型')
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('当前权限')
    await dropConversation('需求分析', 'chat-b')
    await page.getByRole('textbox', { name: '名称', exact: true }).hover()
    await expect.element(await nodeDetails('需求分析')).toHaveTextContent('节点默认模型')
    await dropConversation('需求分析', 'chat-a')
    await page.getByRole('textbox', { name: '名称', exact: true }).hover()
    await expect.element(await nodeDetails('需求分析')).toHaveTextContent('对话后续模型')
    expect(composerDraft.modelId).toBe('user-model')
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
  })

  it('submits existing conversation IDs and unbound nodes once after flushing pending conversation writes', async () => {
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-a')
    await page.getByRole('button', { name: '激活', exact: true }).click()
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
    expect(saves[0][0]).not.toHaveProperty('projectId')
    expect(onBeforeCommit.mock.invocationCallOrder[0]).toBeLessThan(
      service.request.mock.invocationCallOrder.at(-1)!
    )
    await expect
      .element(page.getByRole('button', { name: '配置 产品交付流程', exact: true }))
      .toBeVisible()
  })

  it('prevents a conversation being assigned to a second workflow or a second node, including drop actions', async () => {
    instances = [
      instance('other', '另一组工作流', [{ nodeId: 'analysis', conversationId: 'chat-b' }])
    ]
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-b')
    await expect
      .element(page.getByRole('dialog'))
      .toHaveTextContent('已经加入其他工作流“另一组工作流”')
    expect(page.getByRole('alert').query()).toBeNull()
    expect(page.getByRole('button', { name: '刷新版本' }).query()).toBeNull()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-assignment-conflict.png'
    })
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expectBoundCount(0)
    await dropConversation('需求分析', 'chat-a')
    await dropConversation('开发交付', 'chat-a')
    await expect
      .element(page.getByRole('dialog'))
      .toHaveTextContent('已分配给当前工作流的“需求分析”')
    await page.getByRole('button', { name: '知道了', exact: true }).click()
    await expectBoundCount(1)
    await dropConversation('开发交付', 'missing-chat')
    await expect.element(page.getByRole('alert')).toHaveTextContent('这个对话当前不可用')
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
    await page.getByRole('button', { name: '激活', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await expect.element(dialog).toHaveTextContent('这个对话已经加入其他工作流')
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
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.element(page.getByRole('alert')).toHaveTextContent('工作流已保存')
    await page.getByRole('button', { name: '重试同步' }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(2)
    expect(onCommitted.mock.calls[0][0]).toEqual(onCommitted.mock.calls[1][0])
    expect(
      service.request.mock.calls.filter(([input]) => input.operation === 'saveInstance')
    ).toHaveLength(1)
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
    await page.getByRole('button', { name: '刷新版本', exact: true }).click()
    await expect.element(page.getByRole('status')).toHaveTextContent('已加载最新版本')
    await expectBoundCount(1)
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({
      expectedTemplateRevision: 2,
      bindings: [{ nodeId: 'analysis', conversationId: 'chat-a' }]
    })
  })

  it('opens existing assignments, makes running instances read-only, and removes only the workflow', async () => {
    instances = [
      {
        ...instance('live', '运行工作流', [{ nodeId: 'analysis', conversationId: 'chat-a' }]),
        running: true
      },
      {
        ...instance('idle', '可移除工作流', [{ nodeId: 'analysis', conversationId: 'chat-b' }]),
        color: WORKFLOW_COLORS[1]
      }
    ]
    mount()
    await page.getByRole('button', { name: '配置 运行工作流' }).click()
    await expect.element(page.getByText('这个工作流正在运行，暂时不能修改对话绑定。')).toBeVisible()
    await expect.element(page.getByRole('button', { name: '激活' })).toBeDisabled()
    await dropConversation('需求分析', 'chat-c')
    await expect.element(agent('需求分析')).toHaveTextContent('产品需求讨论')
    await agent('需求分析').click()
    await expect
      .element(page.getByRole('button', { name: '解除对话分配', exact: true }))
      .toBeDisabled()
    await page.getByRole('button', { name: '取消', exact: true }).click()
    await page.getByRole('button', { name: '移除工作流 可移除工作流' }).click()
    await expect.element(page.getByRole('dialog')).toHaveTextContent('所有对话都会保留')
    await page.getByRole('button', { name: '移除', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'deleteInstance')?.[0]
    ).toEqual({ operation: 'deleteInstance', id: 'idle', expectedRevision: 1 })
  })

  it('toggles node selection and clears only the selected assignment before saving one replacement', async () => {
    const originalConversations = structuredClone(conversations)
    instances = [
      instance('existing', '已配置工作流', [
        { nodeId: 'analysis', conversationId: 'chat-a' },
        { nodeId: 'delivery', conversationId: 'chat-b' }
      ])
    ]
    mount()
    await page.getByRole('button', { name: '配置 已配置工作流' }).click()
    const remove = page.getByRole('button', { name: '解除对话分配', exact: true })
    expect(remove.query()).toBeNull()
    await page.getByRole('textbox', { name: '名称', exact: true }).click()
    await agent('需求分析').click()
    expect(document.activeElement).toBe(agent('需求分析').element())
    await userEvent.keyboard('x')
    await expect
      .element(page.getByRole('textbox', { name: '名称', exact: true }))
      .toHaveValue('已配置工作流')
    await expect.element(agent('需求分析')).toHaveAttribute('aria-pressed', 'true')
    await expect.element(remove).toBeVisible()
    expect(page.getByRole('complementary', { name: '选择对话' }).query()).toBeNull()
    expect(page.getByRole('dialog').query()).toBeNull()
    expect(page.getByRole('tooltip').query()).toBeNull()
    const paletteBounds = page
      .getByRole('button', { name: '工作流颜色', exact: true })
      .element()
      .getBoundingClientRect()
    const removeBounds = remove.element().getBoundingClientRect()
    expect(removeBounds.left).toBeGreaterThanOrEqual(paletteBounds.right)
    expect(removeBounds.left - paletteBounds.right).toBeLessThan(24)
    await agent('需求分析').click()
    await expect.element(agent('需求分析')).toHaveAttribute('aria-pressed', 'false')
    expect(remove.query()).toBeNull()
    await agent('需求分析').click()
    await remove.click()
    await expectBoundCount(1)
    expect(remove.query()).toBeNull()
    await expect.element(agent('需求分析')).not.toHaveClass('is-bound')
    expect(conversations).toEqual(originalConversations)
    expect(
      service.request.mock.calls.every(([input]) =>
        ['list', 'listInstances'].includes(input.operation)
      )
    ).toBe(true)
    await page.getByRole('button', { name: '激活', exact: true }).click()
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
    expect(instances[0].bindings).toEqual([
      { nodeId: 'analysis', conversationId: 'created-analysis' },
      { nodeId: 'delivery', conversationId: 'chat-b' }
    ])
    expect(conversations).toEqual(originalConversations)
  })

  it('collapses workflow colors into a single-row palette that closes on selection, outside click and Escape', async () => {
    mount()
    await begin()
    const palette = page.getByRole('button', { name: '工作流颜色', exact: true })
    const popover = page.getByRole('dialog', { name: '工作流颜色', exact: true })
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
    await page.getByRole('button', { name: '激活', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'saveInstance')?.[0]
    ).toMatchObject({ color: '#B57BE8' })
  })

  it('shows node permissions, model and task only after a one-second pointer hover and cleans up pending timers', async () => {
    const view = mount()
    await begin()
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    await agent('需求分析').hover()
    await vi.advanceTimersByTimeAsync(999)
    expect(page.getByRole('tooltip').query()).toBeNull()
    await vi.advanceTimersByTimeAsync(1)
    const tooltip = page.getByRole('tooltip')
    await expect.element(tooltip).toHaveTextContent('节点模型')
    await expect.element(tooltip).toHaveTextContent('节点默认模型')
    await expect.element(tooltip).toHaveTextContent('节点权限')
    await expect.element(tooltip).toHaveTextContent('自定义权限')
    await expect.element(tooltip).toHaveTextContent('分析需求')
    await page.getByRole('textbox', { name: '名称', exact: true }).hover()
    await expect.element(tooltip).not.toBeInTheDocument()
    await agent('需求分析').hover()
    await vi.advanceTimersByTimeAsync(400)
    await page.getByRole('textbox', { name: '名称', exact: true }).hover()
    await vi.advanceTimersByTimeAsync(1000)
    expect(tooltip.query()).toBeNull()
    await agent('需求分析').hover()
    await vi.advanceTimersByTimeAsync(400)
    await (await view).unmount()
    await vi.advanceTimersByTimeAsync(1000)
    expect(tooltip.query()).toBeNull()
  })

  it('renders the full-width binding graph with high-contrast bound nodes and shared zoom controls', async () => {
    mount()
    await begin()
    await dropConversation('需求分析', 'chat-b')
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-binding-dark.png'
    })
    expect(document.querySelector('.project-workflows__chooser')).toBeNull()
    const boundStyle = getComputedStyle(agent('需求分析').element())
    const unboundStyle = getComputedStyle(agent('开发交付').element())
    expect(boundStyle.backgroundColor).not.toBe(unboundStyle.backgroundColor)
    expect(boundStyle.borderTopColor).toBe('rgb(79, 143, 234)')
    expect(document.querySelector('.project-workflows__header')).toBeNull()
    expect(document.querySelector('.project-workflows__footer')).toBeNull()
    expect(document.querySelector('.project-workflows__binding-help')).toBeNull()
    const toolbar = document
      .querySelector('.project-workflows__binding-toolbar')!
      .getBoundingClientRect()
    const nameInput = page
      .getByRole('textbox', { name: '名称', exact: true })
      .element()
      .getBoundingClientRect()
    const cancel = page
      .getByRole('button', { name: '取消', exact: true })
      .element()
      .getBoundingClientRect()
    const confirm = page
      .getByRole('button', { name: '激活', exact: true })
      .element()
      .getBoundingClientRect()
    expect(
      Math.abs(nameInput.top + nameInput.height / 2 - confirm.top - confirm.height / 2)
    ).toBeLessThan(2)
    expect(confirm.left).toBeGreaterThan(cancel.left)
    expect(cancel.left).toBeGreaterThan(1000)
    expect(toolbar.height).toBeLessThan(70)
    const canvas = document.querySelector('.workflow-binding-canvas')!.getBoundingClientRect()
    expect(canvas.height).toBeGreaterThan(350)
    expect(canvas.width).toBeGreaterThan(1200)
    const zoomControls = document.querySelector('.workflow-canvas-controls')!
    const percentage = zoomControls.querySelector('.workflow-canvas-controls__percentage')!
    expect(getComputedStyle(percentage).fontSize).toBe('10px')
    for (const icon of zoomControls.querySelectorAll('svg')) {
      expect(getComputedStyle(icon).width).toBe('14px')
      expect(getComputedStyle(icon).height).toBe('14px')
    }
    expect(zoomControls.querySelector('.workflow-canvas-controls__separator')).not.toBeNull()
    const startingZoom = percentage.textContent
    await page.getByRole('button', { name: '放大', exact: true }).click()
    await expect.poll(() => percentage.textContent).not.toBe(startingZoom)
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await expect.poll(() => percentage.textContent).toBe(startingZoom)

    expect(canvas.top).toBeGreaterThanOrEqual(toolbar.bottom)
    expect(canvas.height).toBeGreaterThan(800)
    expect(document.querySelector('.project-workflows')!.scrollWidth).toBeLessThanOrEqual(1440)
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-binding-light.png'
    })
    await page.viewport(1000, 800)
    await expect
      .poll(() => document.querySelector('.project-workflows')!.scrollWidth)
      .toBeLessThanOrEqual(1000)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-binding-narrow.png'
    })
  })
})
