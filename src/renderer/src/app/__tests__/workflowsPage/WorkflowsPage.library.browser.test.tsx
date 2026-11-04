import { page } from 'vitest/browser'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { WORKFLOW_COLORS } from '../../../features/workflows/project/projectWorkflowText'
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

  it.each(['zh-CN', 'en-US'] as const)(
    'explains structured unavailable-model failures in %s without changing organization state',
    async (language) => {
      service.language = language
      instances = [
        {
          ...instance('blocked', 'Research', [
            { nodeId: 'analysis', conversationId: 'chat-a' },
            { nodeId: 'delivery', conversationId: 'chat-b' }
          ]),
          enabled: false
        }
      ]
      const before = structuredClone(instances)
      const original = service.request.getMockImplementation()!
      service.request.mockImplementation(async (input: WorkflowRequest) => {
        if (input.operation !== 'setInstanceEnabled') return original(input)
        throw new HostInvocationError({
          message: 'organization_member_models_unavailable',
          code: -32602,
          data: {
            code: 'organization_member_models_unavailable',
            members: [
              {
                nodeId: 'analysis',
                nodeName: '方法抽取员',
                modelConfigId: 'private-model-id-a',
                modelDisplayName: 'DSflash'
              },
              {
                nodeId: 'delivery',
                nodeName: '机制收集员',
                modelConfigId: 'private-model-id-a',
                modelDisplayName: 'DSflash'
              },
              ...['软件机制抽取员', '适用性评估员', '解析推导员', '数值验证员'].map(
                (nodeName, index) => ({
                  nodeId: `research-${index}`,
                  nodeName,
                  modelConfigId: 'private-model-id-a',
                  modelDisplayName: 'DSflash'
                })
              ),
              {
                nodeId: 'reviewer',
                nodeName: '数学复核员',
                modelConfigId: 'private-model-id-b',
                modelDisplayName: null
              }
            ]
          }
        })
      })
      await mount()
      const zh = language === 'zh-CN'
      const toggle = page.getByRole('switch', {
        name: `${zh ? '启用组织' : 'Enable organization'} Research`,
        exact: true
      })
      await toggle.click()
      const dialog = page.getByRole('alertdialog', {
        name: zh ? '无法启用组织' : 'Could not enable organization',
        exact: true
      })
      await expect.element(dialog).toBeVisible()
      const description = dialog.element().querySelector('p')!
      expect(description.textContent).toContain(
        zh
          ? 'DSflash（不可用）\n受影响成员：方法抽取员、机制收集员'
          : 'DSflash (unavailable)\nAffected members: 方法抽取员, 机制收集员'
      )
      expect(description.textContent?.match(/DSflash/g)).toHaveLength(1)
      expect(description.textContent).toContain(
        zh
          ? '已删除或不可用的模型\n受影响成员：数学复核员'
          : 'Deleted or unavailable model\nAffected members: 数学复核员'
      )
      expect(description.textContent).toContain('数学复核员')
      expect(description.textContent).toContain(
        zh
          ? '请在模型设置中检查并启用该模型，或在组织配置中为这些成员选择可用模型。'
          : 'Check and enable these models in Model settings, or choose available models for these members in Organization settings.'
      )
      expect(getComputedStyle(description).whiteSpace).toBe('pre-wrap')
      for (const hidden of [
        'organization_member_models_unavailable',
        'private-model-id-',
        '-32602',
        '暂时无法切换组织状态',
        'Could not change organization state'
      ])
        expect(dialog.element().textContent).not.toContain(hidden)
      expect(instances).toEqual(before)
      expect(onCommitted).not.toHaveBeenCalled()
      expect(onBeforeCommit).not.toHaveBeenCalled()
      await expect.element(toggle).toHaveAttribute('aria-checked', 'false')
      // Reading a fresh snapshot must not dismiss an unrelated action failure.
      const beforeRecovery = service.request.mock.calls.length
      window.dispatchEvent(new Event('focus'))
      await expect.poll(() => service.request.mock.calls.length).toBeGreaterThan(beforeRecovery)
      await expect.element(dialog).toBeVisible()
      await page.getByRole('button', { name: zh ? '知道了' : 'Got it', exact: true }).click()
      expect(dialog.query()).toBeNull()
      // A later, unrelated failure must not inherit the specific model-error title or content.
      service.request.mockImplementation(async (input: WorkflowRequest) => {
        if (input.operation !== 'setInstanceEnabled') return original(input)
        throw new HostInvocationError({ message: 'private storage failure', code: -32000 })
      })
      await toggle.click()
      const generic = page.getByRole('alertdialog', {
        name: zh ? '暂时无法完成操作' : 'Could not complete this action',
        exact: true
      })
      await expect
        .element(generic)
        .toHaveTextContent(
          zh
            ? '暂时无法切换组织状态，请检查配置后重试。'
            : 'Could not change organization state. Check its configuration and try again.'
        )
      expect(generic.element().textContent).not.toContain('DSflash')
      expect(generic.element().textContent).not.toContain('private storage failure')
    }
  )

  it('keeps long unavailable-model member lists readable and the acknowledgement visible', async () => {
    await page.viewport(400, 640)
    instances = [
      {
        ...instance('blocked', 'Research', [
          { nodeId: 'analysis', conversationId: 'chat-a' },
          { nodeId: 'delivery', conversationId: 'chat-b' }
        ]),
        enabled: false
      }
    ]
    const original = service.request.getMockImplementation()!
    service.request.mockImplementation(async (input: WorkflowRequest) => {
      if (input.operation !== 'setInstanceEnabled') return original(input)
      throw new HostInvocationError({
        message: 'organization_member_models_unavailable',
        code: -32602,
        data: {
          code: 'organization_member_models_unavailable',
          members: Array.from({ length: 40 }, (_, index) => ({
            nodeId: `member-${index}`,
            nodeName: `负责长流程研究及验证的成员${index + 1}`,
            modelConfigId: 'private-model-id',
            modelDisplayName: 'DSflash'
          }))
        }
      })
    })
    await mount()
    await page.getByRole('switch', { name: '启用组织 Research', exact: true }).click()
    const dialog = page.getByRole('alertdialog', { name: '无法启用组织', exact: true })
    await expect.element(dialog).toBeVisible()
    const description = dialog.element().querySelector('p')!
    expect(description.textContent).toContain('负责长流程研究及验证的成员40')
    expect(description.scrollHeight).toBeGreaterThan(description.clientHeight)
    expect(description.scrollWidth).toBeLessThanOrEqual(description.clientWidth + 1)
    await expect.element(dialog.getByRole('button', { name: '知道了', exact: true })).toBeVisible()
    expect(dialog.element().getBoundingClientRect().bottom).toBeLessThanOrEqual(640)
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
      path: '../../../../../../.cache/workflow-authoring/workflow-running-lights-dark.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/workflow-running-lights-light.png'
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
      path: '../../../../../../.cache/workflow-authoring/global-workflow-cards-compact-dark.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables()))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../../.cache/workflow-authoring/global-workflow-cards-compact-light.png'
    })
  })
})
