import type { WorkflowDefinition, WorkflowInstance } from '@mycopilot/protocol'
import { page, userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../config/themes/classic'
import type { ChatConversation } from '../../features/chat/chatTypes'
import type { ConversationAttentionById } from '../../features/chat/useConversationAttention'
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
  transmissions: [] as import('@mycopilot/protocol').WorkflowRuntimeEvent[]
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
  viewport: { x: 0, y: 0, zoom: 1 },
  nodes: [
    agent('plan', '任务架构师', 280, 160),
    agent('build', '开发工程师', 710, 60),
    agent('review', 'Review 专家', 710, 260)
  ]
}
const instance: WorkflowInstance = {
  id: 'workflow',
  name: '新功能开发',
  templateId: graph.id,
  templateRevision: 1,
  definition: structuredClone(graph),
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
      instance={{ ...workflow, definition }}
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
  onBack.mockReset()
  onOpenConversation.mockReset()
})

describe('organization read-only monitor', () => {
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
  it('preserves independent member coordinates and opens only bound conversations', async () => {
    const screen = await render(renderPage())
    expect(screen.container.querySelectorAll('.workflow-node')).toHaveLength(graph.nodes.length)
    expect(screen.container.querySelectorAll('.workflow-node--gate')).toHaveLength(0)
    expect(screen.container.querySelectorAll('.workflow-edge')).toHaveLength(0)
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
    await screen.getByRole('button', { name: '返回组织', exact: true }).click()
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
    expect(screen.container.querySelector('.workflow-monitor__transmission')).toBeNull()
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
    await screen.getByRole('button', { name: '双击打开对话 · 开发设置页面', exact: true }).click()
    const panel = screen.getByRole('complementary', { name: '节点看板' })
    await expect.element(panel).toBeVisible()
    expect(panel.element().querySelector('header')?.textContent).not.toContain('等待交互')
    expect(panel.element().querySelector('header')?.textContent).not.toContain('等待批准')
    activity.waitingApproval = new Set(['chat-build'])
    await screen.rerender(
      renderPage(instance, {
        'chat-build': { waitingApproval: false, waitingAnswer: false, unread: false }
      })
    )
    expect(build.dataset.waiting).toBe('approval')
  })

  it('keeps observing real activity when the organization is disabled and supports read-only zoom', async () => {
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
    await screen.getByRole('button', { name: '放大', exact: true }).click()
    expect(stage.style.transform).not.toBe(originalTransform)
    await screen.getByRole('button', { name: '适应画布', exact: true }).click()
    expect(stage.style.transform).toBe(originalTransform)
  })
  it('shows nested department frames from the organization definition with runtime mailboxes', async () => {
    const definition: WorkflowDefinition = {
      ...graph,
      departments: [
        {
          id: 'engineering',
          name: '工程部',
          parentId: null,
          x: 150,
          y: 0,
          width: 900,
          height: 450
        },
        {
          id: 'review-team',
          name: '评审组',
          parentId: 'engineering',
          x: 650,
          y: 200,
          width: 340,
          height: 200
        }
      ]
    }
    const view = await render(renderPage(instance, undefined, conversations, definition))
    await expect.element(view.getByLabelText('工程部 · 1 级部门')).toBeVisible()
    await expect.element(view.getByLabelText('评审组 · 2 级部门')).toBeVisible()
    expect(view.container.querySelectorAll('.workflow-monitor__department')).toHaveLength(2)
    await page.screenshot({ path: '../../../../../.cache/organizations/live-departments.png' })
    expect(view.container.querySelector('.workflow-node__rank')).toBeNull()
    expect(view.container.querySelector('.workflow-node__mailbox')).not.toBeNull()
  })
})
