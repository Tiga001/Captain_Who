import { page } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
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
import '../../styles/global.css'

const service = vi.hoisted(() => ({ request: vi.fn() }))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: service.request }))
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
  onCommitted = vi.fn<(response: WorkflowResponse) => Promise<void>>().mockResolvedValue(undefined)
  onBeforeCommit = vi
    .fn<(conversationIds: readonly string[]) => Promise<void>>()
    .mockResolvedValue(undefined)
  service.request
    .mockReset()
    .mockImplementation(async (input: WorkflowRequest): Promise<WorkflowResponse> => {
      if (input.operation === 'saveInstance') {
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
      return {
        records: [record],
        instances,
        issues: [],
        affectedConversationIds: input.operation === 'saveInstance' ? ['chat-a'] : []
      }
    })
})

function mount(extra: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) {
  return render(
    <div style={{ width: '100vw', height: '100vh' }}>
      <WorkflowsPage
        conversations={conversations}
        projects={projects}
        onNewTemplate={vi.fn()}
        onCommitted={onCommitted}
        onBeforeCommit={onBeforeCommit}
        {...extra}
      />
    </div>
  )
}

async function begin() {
  await expect.element(page.getByRole('heading', { name: '工作流', exact: true })).toBeVisible()
  const activate = document.querySelector(
    '.project-workflows__header button.workflow-button--primary'
  ) as HTMLButtonElement
  await expect.poll(() => activate.disabled).toBe(false)
  activate.click()
  await page.getByRole('button', { name: '产品交付流程 需求分析、开发与验收' }).click()
}

describe('global workflow management', () => {
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
      .element(page.getByRole('button', { name: '激活工作流', exact: true }))
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
    await expect.element(page.getByRole('heading', { name: '绑定对话', exact: true })).toBeVisible()
  })

  it('stages cross-project bindings and cancels without modifying real conversations', async () => {
    const onDirtyChange = vi.fn()
    mount({ onDirtyChange })
    await begin()
    await page.getByRole('button', { name: '界面设计讨论 设计项目 · 已有对话模型' }).click()
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('界面设计讨论')
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('节点默认模型')
    await expect
      .element(
        page.getByText(
          '确认后使用节点预设的模型和权限；正在运行的轮次保持原设置，下轮生效。之后可在对话里独立调整。'
        )
      )
      .toBeVisible()
    await expect.element(page.getByText('已绑定 1 个 · 将新建 1 个对话')).toBeVisible()
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
    await page.getByRole('button', { name: '管理工作流 已配置工作流' }).click()
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('对话后续模型')
    await expect
      .element(page.getByText('当前绑定保持不变，将保留对话现有的模型和权限。'))
      .toBeVisible()
    await page.getByRole('button', { name: '界面设计讨论 设计项目 · 已有对话模型' }).click()
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('节点默认模型')
    await page.getByRole('button', { name: '产品需求讨论 产品项目 · 对话后续模型' }).click()
    await expect
      .element(page.getByRole('button', { name: '绑定对话 · 需求分析' }))
      .toHaveTextContent('对话后续模型')
    expect(composerDraft.modelId).toBe('user-model')
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
  })

  it('submits existing conversation IDs and unbound nodes once after flushing pending conversation writes', async () => {
    mount()
    await begin()
    await page.getByRole('button', { name: '产品需求讨论 产品项目 · 已有对话模型' }).click()
    await page.getByRole('button', { name: '确认激活', exact: true }).click()
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
    await expect.element(page.getByText('已配置', { exact: true })).toBeVisible()
  })

  it('prevents a conversation being assigned to a second workflow or a second node, including drop actions', async () => {
    instances = [
      instance('other', '另一组工作流', [{ nodeId: 'analysis', conversationId: 'chat-b' }])
    ]
    mount()
    await begin()
    await expect
      .element(page.getByRole('button', { name: /界面设计讨论.*已加入 另一组工作流/ }))
      .toBeDisabled()
    await page.getByRole('button', { name: '产品需求讨论 产品项目 · 已有对话模型' }).click()
    await page.getByRole('button', { name: '绑定对话 · 开发交付' }).click()
    await expect
      .element(page.getByRole('button', { name: /产品需求讨论.*已绑定 需求分析/ }))
      .toBeDisabled()
    const target = document.querySelector('[aria-label="绑定对话 · 开发交付"]')!
    const dataTransfer = new DataTransfer()
    dataTransfer.setData('application/x-captain-workflow-conversation', 'chat-b')
    target.dispatchEvent(new DragEvent('drop', { bubbles: true, dataTransfer }))
    await expect.element(page.getByRole('alert')).toHaveTextContent('已加入 另一组工作流')
    expect(service.request.mock.calls.some(([input]) => input.operation === 'saveInstance')).toBe(
      false
    )
  })

  it('keeps successful activation committed when sidebar refresh fails, then retries only synchronization', async () => {
    onCommitted
      .mockRejectedValueOnce(new Error('Storage refresh failed'))
      .mockResolvedValue(undefined)
    mount()
    await begin()
    await page.getByRole('button', { name: '确认激活', exact: true }).click()
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
    await page.getByRole('button', { name: '产品需求讨论 产品项目 · 已有对话模型' }).click()
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
    await expect.element(page.getByText('已绑定 1 个 · 将新建 0 个对话')).toBeVisible()
    await page.getByRole('button', { name: '确认激活', exact: true }).click()
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
      instance('idle', '可移除工作流', [{ nodeId: 'analysis', conversationId: 'chat-b' }])
    ]
    mount()
    await page.getByRole('button', { name: '管理工作流 运行工作流' }).click()
    await expect.element(page.getByText('这个工作流正在运行，暂时不能修改对话绑定。')).toBeVisible()
    await expect.element(page.getByRole('button', { name: '确认配置' })).toBeDisabled()
    await page.getByRole('button', { name: '返回', exact: true }).click()
    await page.getByRole('button', { name: '移除工作流 可移除工作流' }).click()
    await expect.element(page.getByRole('dialog')).toHaveTextContent('所有对话都会保留')
    await page.getByRole('button', { name: '移除', exact: true }).click()
    await expect.poll(() => onCommitted.mock.calls.length).toBe(1)
    expect(
      service.request.mock.calls.find(([input]) => input.operation === 'deleteInstance')?.[0]
    ).toEqual({ operation: 'deleteInstance', id: 'idle', expectedRevision: 1 })
  })

  it('renders the binding graph and project chooser in a compact native layout', async () => {
    mount()
    await begin()
    await page.getByRole('button', { name: '界面设计讨论 设计项目 · 已有对话模型' }).click()
    await page.screenshot({
      path: '../../../../../.cache/workflow-authoring/global-workflow-binding-dark.png'
    })
    const footer = document.querySelector('.project-workflows__footer')!.getBoundingClientRect()
    const canvas = document.querySelector('.workflow-binding-canvas')!.getBoundingClientRect()
    expect(canvas.height).toBeGreaterThan(350)
    expect(footer.top).toBeGreaterThan(canvas.top)
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
