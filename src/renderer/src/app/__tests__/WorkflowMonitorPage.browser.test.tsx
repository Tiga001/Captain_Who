import type { WorkflowDefinition, WorkflowInstance, WorkflowRule } from '@mycopilot/protocol'
import { page, userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../config/themes/classic'
import type { ChatConversation } from '../../features/chat/chatTypes'
import type { ConversationAttentionById } from '../../features/chat/useConversationAttention'
import { graphFlowLayout } from '../../features/workflows/workflowCanvasGeometry'
import '../../styles/global.css'

const messageService = vi.hoisted(() => ({ request: vi.fn() }))
vi.mock('../../features/workflows/workflowClient', () => ({
  requestWorkflows: messageService.request
}))

const activity = vi.hoisted(() => ({
  running: new Set<string>(),
  waitingApproval: new Set<string>()
}))
const execution = vi.hoisted(() => ({
  snapshot: null as import('@mycopilot/protocol').WorkflowRuntimeSnapshot | null,
  transmissions: [] as import('@mycopilot/protocol').WorkflowRuntimeEvent[],
  completeUserInput: vi.fn().mockResolvedValue(undefined),
  discardFailedInput: vi.fn().mockResolvedValue(undefined)
}))
vi.mock('../../features/workflows/project/useWorkflowExecution', () => ({
  useWorkflowExecution: () => execution
}))
vi.mock('../../features/workflows/project/useWorkflowMonitor', () => ({
  useWorkflowMonitor: () => ({
    runningConversationIds: activity.running,
    waitingApprovalConversationIds: activity.waitingApproval
  })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', resolvedColorScheme: 'dark' })
}))
vi.mock('../../features/auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: { displayName: '大副' } } })
}))

const { WorkflowMonitorPage } = await import('../../features/workflows/project/WorkflowMonitorPage')
const agent = (id: string, name: string, x: number, y: number) => ({
  id,
  name,
  x,
  y,
  kind: 'agent' as const,
  modelConfigId: 'model',
  permissionMode: 'default' as const,
  receives: '',
  task: '完成节点职责，并汇总执行结果',
  delivers: ''
})
const graph: WorkflowDefinition = {
  id: 'template',
  schemaVersion: 1,
  name: '产品协作',
  description: '',
  background: '',
  boundaryPositions: { input: { x: 0, y: 160 } },
  viewport: { x: 0, y: 0, zoom: 1 },
  nodes: [
    agent('plan', '任务架构师', 280, 160),
    {
      id: 'split',
      kind: 'outputGate',
      name: '输出门1',
      x: 560,
      y: 160,
      selection: { mode: 'all', min: 1, max: 2, required: [], groups: [] }
    },
    agent('build', '开发工程师', 710, 60),
    agent('review', 'Review 专家', 710, 260),
    {
      id: 'join',
      kind: 'inputGate',
      name: '输入门1',
      x: 1000,
      y: 160,
      processingMode: 'batch',
      busyPolicy: 'queue'
    },
    { id: 'user', kind: 'user', name: '用户验收', x: 1150, y: 160, task: '' }
  ],
  flows: [
    {
      id: 'start',
      name: 'S1',
      source: { kind: 'boundary' },
      target: { kind: 'node', nodeId: 'plan' }
    },
    {
      id: 'dispatch',
      name: 'S2',
      source: { kind: 'node', nodeId: 'plan' },
      target: { kind: 'node', nodeId: 'split' }
    },
    {
      id: 'implement',
      name: 'S3',
      source: { kind: 'node', nodeId: 'split' },
      target: { kind: 'node', nodeId: 'build' }
    },
    {
      id: 'check',
      name: 'S4',
      source: { kind: 'node', nodeId: 'split' },
      target: { kind: 'node', nodeId: 'review' }
    },
    {
      id: 'result',
      name: 'S5',
      source: { kind: 'node', nodeId: 'build' },
      target: { kind: 'node', nodeId: 'join' }
    },
    {
      id: 'report',
      name: 'S6',
      source: { kind: 'node', nodeId: 'review' },
      target: { kind: 'node', nodeId: 'join' }
    },
    {
      id: 'accept',
      name: 'S7',
      source: { kind: 'node', nodeId: 'join' },
      target: { kind: 'node', nodeId: 'user' }
    },
    {
      id: 'feedback',
      name: 'S8',
      source: { kind: 'node', nodeId: 'user' },
      target: { kind: 'node', nodeId: 'plan' }
    }
  ]
}
const instance: WorkflowInstance = {
  id: 'workflow',
  name: '新功能开发',
  templateId: graph.id,
  templateRevision: 1,
  revision: 1,
  updatedAt: 1,
  enabled: true,
  running: false,
  needsReview: false,
  color: '#26AB94',
  bindings: [
    { nodeId: 'plan', conversationId: 'chat-plan' },
    { nodeId: 'build', conversationId: 'chat-build' },
    { nodeId: 'review', conversationId: 'chat-review' }
  ]
}
const conversations: ChatConversation[] = [
  {
    id: 'chat-plan',
    projectId: null,
    title: '拆解新功能需求',
    modelId: 'model',
    messages: [],
    createdAt: 1,
    updatedAt: 1
  },
  {
    id: 'chat-build',
    projectId: null,
    title: '开发设置页面',
    modelId: 'model',
    messages: [],
    createdAt: 1,
    updatedAt: 1
  },
  {
    id: 'chat-review',
    projectId: null,
    title: '检查实现与测试',
    modelId: 'model',
    messages: [],
    createdAt: 1,
    updatedAt: 1
  }
]
const onBack = vi.fn()
const onOpenConversation = vi.fn()
const renderPage = (
  workflow = instance,
  attention?: ConversationAttentionById,
  chats = conversations,
  definition = graph
) => (
  <div style={{ width: '100vw', height: '100vh' }}>
    <WorkflowMonitorPage
      instance={workflow}
      graph={definition}
      conversations={chats}
      conversationAttention={attention}
      models={[{ id: 'model', displayName: 'Qwen3.8MAX', execution: { status: 'available' } }]}
      onBack={onBack}
      onOpenConversation={onOpenConversation}
    />
  </div>
)
const waitForCanvasFit = async (container: HTMLElement) => {
  const canvas = container.querySelector<HTMLElement>('.workflow-monitor__canvas')!
  const extent = container.querySelector<HTMLElement>('.workflow-binding-canvas__extent')!
  await expect.poll(() => extent.style.width).toBe(`${canvas.clientWidth}px`)
}

beforeEach(async () => {
  await page.viewport(1440, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
    document.documentElement.style.setProperty(key, value)
  messageService.request.mockReset().mockImplementation(async (request) => ({
    records: [],
    issues: [],
    nodeMessages: {
      instanceId: request.instanceId,
      nodeId: request.nodeId,
      messages: [],
      nextBeforeSequence: null
    }
  }))
  activity.running = new Set()
  activity.waitingApproval = new Set()
  execution.snapshot = null
  execution.transmissions = []
  execution.completeUserInput.mockClear()
  execution.discardFailedInput.mockClear()
  onBack.mockReset()
  onOpenConversation.mockReset()
})

describe('workflow read-only monitor', () => {
  it('shows stopped queues, batch arrivals and paginated messages without navigating on a single click', async () => {
    const definition = structuredClone(graph)
    definition.flows.find((flow) => flow.id === 'accept')!.target = {
      kind: 'node',
      nodeId: 'build'
    }
    const message = (index: number) => ({
      id: `message-${index}`,
      instanceId: instance.id,
      workflowName: instance.name,
      sourceNodeId: 'review',
      sourceNodeName: 'reviewer',
      sourceConversationId: 'chat-review',
      sourceConversationTitle: 'reviewer',
      targetNodeId: 'build',
      flowId: 'report',
      pathFlowIds: ['report', 'accept'],
      content: `真实消息 ${index}`,
      createdAt: index
    })
    const waiting = {
      id: 'input-1',
      instanceId: instance.id,
      nodeId: 'build',
      conversationId: 'chat-build',
      executionVersion: 'v1',
      content: '',
      messages: [message(1), message(2)],
      busyPolicy: 'queue' as const,
      status: 'paused' as const,
      runId: null,
      deliveryId: null,
      createdAt: 1,
      error: null
    }
    execution.snapshot = {
      instanceId: instance.id,
      sequence: 1,
      inputs: [waiting],
      events: [],
      pendingMessages: [3, 4, 5, 6].map(message),
      pausedConversationIds: ['chat-build'],
      inputRuns: []
    }
    messageService.request.mockImplementation(async (request) => ({
      records: [],
      issues: [],
      nodeMessages: {
        instanceId: instance.id,
        nodeId: request.nodeId,
        messages: request.beforeSequence
          ? [
              {
                sequence: 1,
                message: message(1),
                inputId: 'input-1',
                status: 'paused',
                runStatus: null,
                error: null
              }
            ]
          : [
              {
                sequence: 6,
                message: message(6),
                inputId: null,
                status: 'collecting',
                runStatus: null,
                error: null
              }
            ],
        nextBeforeSequence: request.beforeSequence ? null : 6
      }
    }))
    const screen = await render(renderPage(instance, undefined, conversations, definition))
    const indicator = screen.container.querySelector('[data-queue-count]')!
    expect(indicator.getAttribute('data-queue-count')).toBe('6')
    expect(indicator.hasAttribute('data-overloaded')).toBe(false)
    expect(indicator.querySelector('text')?.textContent).toBe('6')
    expect(indicator.querySelectorAll('[data-filled="true"]')).toHaveLength(2)
    expect(indicator.querySelectorAll('.workflow-monitor__queue-layer')).toHaveLength(3)
    await screen.getByRole('button', { name: '双击打开对话 · 开发设置页面', exact: true }).click()
    const panel = screen.getByRole('complementary', { name: '节点看板' })
    await expect.element(panel).toHaveTextContent('被停止')
    await expect.element(panel).toHaveTextContent('向对应对话手动发送消息可恢复')
    await expect.element(panel).toHaveTextContent('4 条已到达')
    await expect.element(panel).toHaveTextContent('等待到达')
    await expect.element(panel).toHaveTextContent('真实消息 6')
    expect(onOpenConversation).not.toHaveBeenCalled()
    await panel.getByRole('button', { name: '加载更早消息' }).click()
    await expect.element(panel).toHaveTextContent('真实消息 1')
    expect(messageService.request).toHaveBeenCalledWith({
      operation: 'nodeMessages',
      instanceId: instance.id,
      nodeId: 'build',
      beforeSequence: 6
    })
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-node-queue-panel.png'
    })
    await panel.getByRole('button', { name: '收起节点看板' }).click()
    await expect.element(panel).not.toBeInTheDocument()
    execution.snapshot = {
      ...execution.snapshot!,
      sequence: 2,
      inputs: [],
      pendingMessages: [message(3)],
      pausedConversationIds: []
    }
    await screen.rerender(renderPage(instance, undefined, conversations, definition))
    expect(indicator.getAttribute('data-queue-count')).toBe('1')
    expect(indicator.hasAttribute('data-overloaded')).toBe(false)
    expect(indicator.querySelectorAll('[data-filled="true"]')).toHaveLength(1)
  })

  it('pans a fitted graph and zooms around the mouse without editing the graph', async () => {
    const before = structuredClone(graph)
    const screen = await render(renderPage())
    await waitForCanvasFit(screen.container)
    const canvas = screen.container.querySelector<HTMLElement>('.workflow-monitor__canvas')!
    const stage = screen.container.querySelector<HTMLElement>('.workflow-canvas__stage')!
    const initialLeft = parseFloat(stage.style.left)
    const initialTop = parseFloat(stage.style.top)
    const initialZoom = stage.style.transform
    const capture = vi.spyOn(canvas, 'setPointerCapture').mockImplementation(() => undefined)
    const pointer = (type: string, x: number, y: number) =>
      canvas.dispatchEvent(
        new PointerEvent(type, {
          bubbles: true,
          cancelable: true,
          pointerId: 1,
          button: 0,
          clientX: x,
          clientY: y
        })
      )
    pointer('pointerdown', 100, 100)
    pointer('pointermove', 220, 180)
    await expect.poll(() => parseFloat(stage.style.left)).toBeCloseTo(initialLeft + 120)
    expect(parseFloat(stage.style.top)).toBeCloseTo(initialTop + 80)
    pointer('pointercancel', 220, 180)
    pointer('pointermove', 350, 350)
    await expect.poll(() => canvas.classList.contains('is-panning')).toBe(false)
    expect(parseFloat(stage.style.left)).toBeCloseTo(initialLeft + 120)
    capture.mockRestore()
    const rect = canvas.getBoundingClientRect()
    const point = { x: 320, y: 260 }
    const oldZoom = Number(stage.style.transform.match(/scale\((.+)\)/)![1])
    const graphPoint = {
      x: (point.x - parseFloat(stage.style.left)) / oldZoom,
      y: (point.y - parseFloat(stage.style.top)) / oldZoom
    }
    const wheel = new WheelEvent('wheel', {
      bubbles: true,
      cancelable: true,
      deltaY: -120,
      clientX: rect.left + point.x,
      clientY: rect.top + point.y
    })
    canvas.dispatchEvent(wheel)
    expect(wheel.defaultPrevented).toBe(true)
    await expect.poll(() => stage.style.transform).not.toBe(initialZoom)
    const newZoom = Number(stage.style.transform.match(/scale\((.+)\)/)![1])
    expect(newZoom).toBeGreaterThan(oldZoom)
    expect(graphPoint.x * newZoom + parseFloat(stage.style.left)).toBeCloseTo(point.x, 2)
    expect(graphPoint.y * newZoom + parseFloat(stage.style.top)).toBeCloseTo(point.y, 2)
    canvas.dispatchEvent(
      new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 100000 })
    )
    await expect.poll(() => stage.style.transform).toBe('scale(0.15)')
    await screen.getByRole('button', { name: '适应画布', exact: true }).click()
    expect(stage.style.transform).toBe(initialZoom)
    expect(parseFloat(stage.style.left)).toBe(initialLeft)
    expect(parseFloat(stage.style.top)).toBe(initialTop)
    expect(graph).toEqual(before)
    expect(onOpenConversation).not.toHaveBeenCalled()
  })

  it('shows input policies and output selections in delayed gate tooltips', async () => {
    const screen = await render(renderPage())
    await screen.getByRole('group', { name: '输入门1', exact: true }).hover()
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('按批次处理')
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('所有入流都有消息后')
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('排队')
    await screen.getByRole('group', { name: '输出门1', exact: true }).hover()
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('全部输出')
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('S3 → 开发工程师')
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('S4 → Review 专家')
    expect(onOpenConversation).not.toHaveBeenCalled()
  })

  it.each<[{ rule: WorkflowRule; expected: string[] }]>([
    [
      {
        rule: { mode: 'exact', min: 2, max: 2, required: [], groups: [] },
        expected: ['选择 N 条', '数量', '2']
      }
    ],
    [
      {
        rule: { mode: 'range', min: 1, max: 2, required: [], groups: [] },
        expected: ['数量范围', '最少数量', '最多数量']
      }
    ],
    [
      {
        rule: {
          mode: 'custom',
          min: 0,
          max: 0,
          required: ['implement'],
          groups: [{ id: 'g', flowIds: ['check'], min: 0, max: 1 }]
        },
        expected: ['自定义组合', '必选', 'S3 → 开发工程师', '选择分组 1 · 0–1', 'S4 → Review 专家']
      }
    ]
  ])('shows concrete output gate constraints for $rule.mode', async ({ rule, expected }) => {
    const definition = structuredClone(graph)
    const gate = definition.nodes.find((node) => node.id === 'split')!
    if (gate.kind !== 'outputGate') throw new Error('Expected output gate')
    gate.selection = rule
    const screen = await render(renderPage(instance, undefined, conversations, definition))
    await screen.getByRole('group', { name: '输出门1', exact: true }).hover()
    for (const content of expected)
      await expect.element(page.getByRole('tooltip')).toHaveTextContent(content)
    if (rule.mode === 'custom')
      await page.screenshot({
        path: '../../../../../.cache/workflow-authoring/workflow-monitor-gate-tooltip.png'
      })
  })

  it('requires explicit confirmation before skipping a failed input while keeping chat navigation separate', async () => {
    const input: import('@mycopilot/protocol').WorkflowRuntimeInput = {
      id: 'failed-input',
      instanceId: instance.id,
      nodeId: 'build',
      conversationId: 'chat-build',
      executionVersion: 'v1',
      content: '',
      messages: [],
      busyPolicy: 'queue',
      status: 'failed',
      runId: 'run',
      deliveryId: 'delivery',
      createdAt: 1,
      error: 'Delivery interrupted'
    }
    execution.snapshot = { instanceId: instance.id, sequence: 1, inputs: [input], events: [] }
    const screen = await render(renderPage())
    await screen.getByRole('button', { name: '处理投递失败' }).click()
    expect(execution.discardFailedInput).not.toHaveBeenCalled()
    await page.getByRole('button', { name: '跳过此来信' }).click()
    expect(execution.discardFailedInput).toHaveBeenCalledExactlyOnceWith('failed-input')
    expect(onOpenConversation).not.toHaveBeenCalled()
  })
  it('shows received user work and confirms only that pending input', async () => {
    const input: import('@mycopilot/protocol').WorkflowRuntimeInput = {
      id: 'user-input',
      instanceId: instance.id,
      nodeId: 'user',
      conversationId: null,
      executionVersion: 'v1',
      content: 'Review the two results',
      messages: [],
      busyPolicy: 'queue',
      status: 'waiting_user',
      runId: null,
      deliveryId: null,
      createdAt: 1,
      error: null
    }
    execution.snapshot = { instanceId: instance.id, sequence: 1, inputs: [input], events: [] }
    execution.transmissions = [
      {
        instanceId: instance.id,
        sequence: 1,
        inputId: input.id,
        flowIds: ['accept'],
        kind: 'waiting_user',
        createdAt: Date.now()
      }
    ]
    const screen = await render(renderPage())
    expect(screen.container.querySelectorAll('.workflow-monitor__transmission')).toHaveLength(1)
    await screen.getByRole('button', { name: '用户验收 · 等待用户操作' }).click()
    await expect.element(page.getByRole('dialog', { name: '等待用户操作' })).toBeVisible()
    await page.getByRole('button', { name: '我已完成' }).click()
    expect(execution.completeUserInput).toHaveBeenCalledExactlyOnceWith('user-input')
    expect(onOpenConversation).not.toHaveBeenCalled()
  })
  it('preserves the template topology and coordinates and opens only bound conversations', async () => {
    const screen = await render(renderPage())
    const { geometries } = graphFlowLayout(graph)
    expect(screen.container.querySelectorAll('.workflow-node')).toHaveLength(graph.nodes.length + 1)
    expect(screen.container.querySelectorAll('.workflow-node--gate')).toHaveLength(2)
    expect(screen.container.querySelectorAll('.workflow-edge')).toHaveLength(graph.flows.length)
    for (const flow of graph.flows) {
      const path = screen.container.querySelector(
        `[data-flow-id="${flow.id}"] .workflow-edge__line`
      )
      expect(path?.getAttribute('d')).toBe(geometries.get(flow.id)?.path)
    }
    const plan =
      screen.container.querySelector<HTMLElement>('[data-node-id="plan"]')!.parentElement!
        .parentElement!
    const build =
      screen.container.querySelector<HTMLElement>('[data-node-id="build"]')!.parentElement!
        .parentElement!
    expect(parseFloat(build.style.left) - parseFloat(plan.style.left)).toBe(430)
    expect(parseFloat(build.style.top) - parseFloat(plan.style.top)).toBe(-100)
    expect(screen.container.querySelector('.workflow-anchor-handle')).toBeNull()
    expect(screen.container.querySelector('input')).toBeNull()
    const conversationNode = screen.getByRole('button', {
      name: '双击打开对话 · 开发设置页面',
      exact: true
    })
    await conversationNode.click()
    expect(onOpenConversation).not.toHaveBeenCalled()
    await userEvent.dblClick(conversationNode)
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-build')
    await screen.getByRole('button', { name: '返回工作流', exact: true }).click()
    expect(onBack).toHaveBeenCalledOnce()
    await screen.rerender(renderPage({ ...instance, bindings: instance.bindings.slice(0, 2) }))
    await expect
      .element(screen.getByRole('button', { name: '未绑定 · Review 专家', exact: true }))
      .not.toBeDisabled()
  })

  it('breathes only on real active nodes without moving any node or synthesizing message flow', async () => {
    const screen = await render(renderPage())
    await waitForCanvasFit(screen.container)
    const snapshot = () =>
      [...screen.container.querySelectorAll('.workflow-node')].map((node) => {
        const box = node.getBoundingClientRect()
        return { left: box.left, top: box.top, width: box.width, height: box.height }
      })
    const before = snapshot()
    activity.running = new Set(['chat-build', 'chat-review'])
    await screen.rerender(renderPage())
    expect(snapshot()).toEqual(before)
    expect(
      [...screen.container.querySelectorAll('.workflow-monitor__node.is-running')].map((node) =>
        node.getAttribute('data-node-id')
      )
    ).toEqual(['build', 'review'])
    const active = screen.container.querySelector<HTMLElement>('[data-node-id="build"]')!
    expect(getComputedStyle(active, '::after').animationName).toBe('workflow-monitor-breathe')
    expect(getComputedStyle(active, '::after').borderTopColor).toBe('rgb(38, 171, 148)')
    const orbit = active.querySelector('.workflow-monitor__node-orbit')!
    const orbitHead = active.querySelector('.workflow-monitor__node-orbit-head')!
    expect(getComputedStyle(orbit).visibility).toBe('visible')
    expect(getComputedStyle(orbit).pointerEvents).toBe('none')
    expect(getComputedStyle(orbitHead).animationName).toBe('workflow-monitor-orbit')
    expect(getComputedStyle(orbitHead).stroke).toBe('rgb(38, 171, 148)')
    expect(orbitHead.getAttribute('pathLength')).toBe('100')
    expect(getComputedStyle(active).animationName).toBe('none')
    expect(screen.container.querySelector('[class*="transmission"]')).toBeNull()
    for (const animation of screen.container.getAnimations({ subtree: true })) {
      animation.pause()
      animation.currentTime = 1100
    }
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-monitor-dark.png'
    })
    for (const [key, value] of Object.entries(
      getFrontendCssVariables(undefined, classicLightTheme)
    ))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-monitor-light.png'
    })
    activity.running = new Set()
    await screen.rerender(renderPage())
    expect(snapshot()).toEqual(before)
    expect(screen.container.querySelector('.workflow-monitor__node.is-running')).toBeNull()
    expect(getComputedStyle(orbit).visibility).toBe('hidden')
    expect(getComputedStyle(orbitHead).animationName).toBe('none')
  })

  it('prioritizes approval over answers, keeps unread visible, and clears settled states without moving content', async () => {
    const screen = await render(renderPage())
    await waitForCanvasFit(screen.container)
    const snapshot = () =>
      [...screen.container.querySelectorAll('.workflow-node, .workflow-node__copy > *')].map(
        (node) => {
          const box = node.getBoundingClientRect()
          return { left: box.left, top: box.top, width: box.width, height: box.height }
        }
      )
    const before = snapshot()
    activity.running = new Set(['chat-build', 'chat-review'])
    const attention = {
      'chat-build': { waitingApproval: true, waitingAnswer: true, unread: true },
      'chat-review': { waitingApproval: false, waitingAnswer: true, unread: false }
    }
    await screen.rerender(renderPage(instance, attention))
    expect(snapshot()).toEqual(before)
    const build = screen.container.querySelector<HTMLElement>('[data-node-id="build"]')!
    const review = screen.container.querySelector<HTMLElement>('[data-node-id="review"]')!
    expect(build.dataset.waiting).toBe('approval')
    expect(build.querySelector('.workflow-monitor__node-attention')?.textContent).toBe('等待批准')
    expect(build.querySelector('[aria-label="未读消息"]')).not.toBeNull()
    expect(review.dataset.waiting).toBe('answer')
    expect(review.querySelector('.workflow-monitor__node-attention')?.textContent).toBe('等待交互')
    const textRange = document.createRange()
    textRange.selectNodeContents(build.querySelector('.workflow-node__copy > span')!)
    const badge = build.querySelector('.workflow-monitor__node-attention')!.getBoundingClientRect()
    expect(textRange.getBoundingClientRect().right).toBeLessThan(badge.left)
    for (const animation of screen.container.getAnimations({ subtree: true })) {
      animation.pause()
      animation.currentTime = 1300
    }
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-monitor-attention-dark.png'
    })
    for (const [key, value] of Object.entries(
      getFrontendCssVariables(undefined, classicLightTheme)
    ))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/workflow-monitor-attention-light.png'
    })
    await screen.rerender(
      renderPage(instance, {
        ...attention,
        'chat-build': { waitingApproval: false, waitingAnswer: true, unread: true }
      })
    )
    expect(build.dataset.waiting).toBe('answer')
    expect(build.querySelector('.workflow-monitor__node-attention')?.textContent).toBe('等待交互')
    expect(build.querySelector('[aria-label="未读消息"]')).not.toBeNull()
    await screen.rerender(
      renderPage(instance, {
        'chat-build': { waitingApproval: false, waitingAnswer: false, unread: false },
        'chat-review': { waitingApproval: false, waitingAnswer: false, unread: false }
      })
    )
    expect(build.dataset.waiting).toBeUndefined()
    expect(screen.container.querySelector('.workflow-monitor__node-attention')).toBeNull()
    expect(screen.container.querySelector('.workflow-monitor__node-unread')).toBeNull()
    expect(snapshot()).toEqual(before)
    const conversationNode = screen.getByRole('button', {
      name: '双击打开对话 · 开发设置页面',
      exact: true
    })
    await conversationNode.click()
    expect(onOpenConversation).not.toHaveBeenCalled()
    await userEvent.dblClick(conversationNode)
    expect(onOpenConversation).toHaveBeenCalledExactlyOnceWith('chat-build')
  })

  it('combines descendant approvals with conversation attention and supports local state fallback', async () => {
    activity.waitingApproval = new Set(['chat-build'])
    const waitingChats: ChatConversation[] = conversations.map((conversation) =>
      conversation.id === 'chat-build'
        ? {
            ...conversation,
            unreadAt: 123,
            messages: [
              {
                id: 'question',
                role: 'assistant',
                content: '',
                createdAt: 1,
                status: 'pending',
                agentRun: {
                  runId: 'run',
                  status: 'waiting_for_user_input',
                  toolDefinitions: [],
                  toolCalls: [],
                  toolResults: [],
                  approvals: [],
                  fileChangeProposals: [],
                  timeline: []
                }
              }
            ]
          }
        : conversation
    )
    const screen = await render(renderPage(instance, undefined, waitingChats))
    const build = screen.container.querySelector<HTMLElement>('[data-node-id="build"]')!
    expect(build.dataset.waiting).toBe('approval')
    expect(build.querySelector('[aria-label="未读消息"]')).not.toBeNull()
    activity.waitingApproval = new Set()
    await screen.rerender(renderPage(instance, undefined, waitingChats))
    expect(build.dataset.waiting).toBe('answer')
    await screen.rerender(
      renderPage(
        instance,
        { 'chat-build': { waitingApproval: false, waitingAnswer: false, unread: false } },
        waitingChats
      )
    )
    expect(build.dataset.waiting).toBeUndefined()
    expect(build.querySelector('[aria-label="未读消息"]')).toBeNull()
    activity.waitingApproval = new Set(['chat-build'])
    await screen.rerender(
      renderPage(instance, {
        'chat-build': { waitingApproval: false, waitingAnswer: false, unread: false }
      })
    )
    expect(build.dataset.waiting).toBe('approval')
  })

  it('keeps observing real activity when the workflow is disabled and supports read-only zoom', async () => {
    activity.running = new Set(['chat-plan'])
    const screen = await render(renderPage({ ...instance, enabled: false }))
    await waitForCanvasFit(screen.container)
    expect(
      screen.container.querySelector('[data-node-id="plan"]')?.classList.contains('is-running')
    ).toBe(true)
    const stage = screen.container.querySelector<HTMLElement>('.workflow-canvas__stage')!
    const originalTransform = stage.style.transform
    await screen.getByRole('button', { name: '100%', exact: true }).click()
    expect(stage.style.transform).toBe('scale(1)')
    expect(stage.style.transform).not.toBe(originalTransform)
    await screen.getByRole('button', { name: '适应画布', exact: true }).click()
    expect(stage.style.transform).toBe(originalTransform)
  })
})
