import { page } from 'vitest/browser'
import { expect, vi, type Mock } from 'vitest'
import { render } from 'vitest-browser-react'
import { useCallback, useEffect, useState } from 'react'
import type { AppProject } from '../../../config/projectConfig'
import type {
  RightSidebarPage,
  RightSidebarPageUpdate
} from '../../../features/rightSidebar/rightSidebarTypes'
import type {
  WorkflowInstance,
  WorkflowRecord,
  WorkflowRequest,
  WorkflowResponse,
  WorkflowRuntimeSnapshot
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../../features/chat/chatTypes'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme } from '../../../config/themes/classic'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import { WORKFLOW_CONVERSATION_DRAG_TYPE } from '../../../features/workflows/project/projectWorkflowText'
import { invalidateWorkflowPages } from '../../../features/workflows/workflowPageCache'
import type { WorkflowsPage as WorkflowsPageComponent } from '../../../features/workflows/project/WorkflowsPage'
import type { WorkflowSidebarPage as WorkflowSidebarPageComponent } from '../../../features/workflows/project/WorkflowSidebarPage'

export interface WorkflowPageTestState {
  record: WorkflowRecord
  extraRecords: WorkflowRecord[]
  instances: WorkflowInstance[]
  conversations: ChatConversation[]
  projects: AppProject[]
  onCommitted: Mock<(response: WorkflowResponse) => Promise<void>>
  onBeforeCommit: Mock<(conversationIds: readonly string[]) => Promise<void>>
}

interface WorkflowPageTestService {
  account: string
  runtimeListeners: Set<(snapshot: WorkflowRuntimeSnapshot) => void>
}

export const conversation = (id: string, title: string, projectId: string): ChatConversation => ({
  id,
  title,
  projectId,
  modelId: 'chat-model',
  messages: [],
  createdAt: 1,
  updatedAt: 1
})

/** Every invocation owns fresh mutable records, projects, and conversations. */
export function createWorkflowPageFixtures(): Pick<
  WorkflowPageTestState,
  'record' | 'conversations' | 'projects'
> {
  return {
    record: {
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
    },
    conversations: [
      conversation('chat-a', '产品需求讨论', 'project-a'),
      conversation('chat-b', '界面设计讨论', 'project-b'),
      conversation('chat-c', '自动化验收讨论', 'project-b')
    ],
    projects: [
      { id: 'project-a', name: '产品项目', folders: [], createdAt: 1 },
      { id: 'project-b', name: '设计项目', folders: [], createdAt: 1 }
    ]
  }
}

export function createWorkflowInstance(
  record: WorkflowRecord,
  id: string,
  name: string,
  bindings: WorkflowInstance['bindings']
): WorkflowInstance {
  return {
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
  }
}

export async function resetWorkflowPageEnvironment(service: WorkflowPageTestService) {
  service.account = 'account-a'
  invalidateWorkflowPages()
  await page.viewport(1440, 900)
  const css = getFrontendCssVariables(undefined, classicDarkTheme)
  for (const [key, value] of Object.entries(css))
    document.documentElement.style.setProperty(key, value)
}

/** Accessors retain the original live-binding behavior when a case replaces its snapshots. */
export function createWorkflowRequestHandler(state: WorkflowPageTestState) {
  const instance = (id: string, name: string, bindings: WorkflowInstance['bindings']) =>
    createWorkflowInstance(state.record, id, name, bindings)
  return async (input: WorkflowRequest): Promise<WorkflowResponse> => {
    if (input.operation === 'saveInstance') {
      if (
        state.instances.some(
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
        enabled: state.instances.find((item) => item.id === input.id)?.enabled ?? true,
        definition: structuredClone(input.definition ?? state.record.definition),
        color: input.color,
        projectId: input.projectId ?? null
      }
      state.instances = [...state.instances.filter((item) => item.id !== input.id), created]
    }
    if (input.operation === 'deleteInstance')
      state.instances = state.instances.filter((item) => item.id !== input.id)
    if (input.operation === 'setInstanceEnabled') {
      const current = state.instances.find((item) => item.id === input.id)!
      if (
        input.enabled &&
        state.instances.some(
          (item) =>
            item.id !== input.id &&
            item.enabled &&
            item.color.toLowerCase() === current.color.toLowerCase()
        )
      )
        throw new Error('workflow_color_in_use')
      state.instances = state.instances.map((item) =>
        item.id === input.id
          ? { ...item, enabled: input.enabled, revision: item.revision + 1 }
          : item
      )
    }
    return {
      records: [state.record, ...state.extraRecords],
      instances: state.instances,
      issues: [],
      affectedConversationIds: input.operation === 'saveInstance' ? ['chat-a'] : []
    }
  }
}

export function createWorkflowPageHarness(
  WorkflowsPage: typeof WorkflowsPageComponent,
  WorkflowSidebarPage: typeof WorkflowSidebarPageComponent,
  state: WorkflowPageTestState,
  service: WorkflowPageTestService
) {
  function renderPage(extra: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) {
    return (
      <ToastProvider>
        <div style={{ width: '100vw', height: '100vh' }}>
          <WorkflowsPage
            conversations={state.conversations}
            projects={state.projects}
            onManageTemplates={vi.fn()}
            onCommitted={state.onCommitted}
            onBeforeCommit={state.onBeforeCommit}
            {...extra}
          />
        </div>
      </ToastProvider>
    )
  }
  function mount(extra: Partial<React.ComponentProps<typeof WorkflowsPage>> = {}) {
    return render(renderPage(extra))
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
  function SidebarWorkflowHarness({
    instanceId = null,
    navigationId = 1,
    chats = state.conversations,
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
            projects={state.projects}
            onCommitted={state.onCommitted}
            onManageTemplates={() => undefined}
            onOpenConversation={onOpenConversation}
          />
        </div>
      </ToastProvider>
    )
  }
  return { renderPage, mount, membersChanged, SidebarWorkflowHarness }
}

export function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
export async function begin() {
  await expect.element(page.getByRole('heading', { name: '组织', exact: true })).toBeVisible()
  const activate = document.querySelector(
    '.project-workflows__header button.workflow-button--primary'
  ) as HTMLButtonElement
  await expect.poll(() => activate.disabled).toBe(false)
  activate.click()
  await chooseTemplate('产品交付流程')
}
export async function chooseTemplate(name: string) {
  await page.getByRole('button', { name: /^模版:/ }).click()
  await page.getByRole('option', { name, exact: true }).click()
}
export function agent(name: string) {
  return page.getByRole('group', { name: `节点 ${name}`, exact: true })
}
export async function dropConversation(nodeName: string, conversationId: string) {
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
export async function expectBoundCount(count: number) {
  await expect.poll(() => document.querySelectorAll('.workflow-node.is-bound').length).toBe(count)
}
