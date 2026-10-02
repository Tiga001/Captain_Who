import { page, userEvent } from 'vitest/browser'
import { useEffect, useReducer } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowDefinition } from '@mycopilot/protocol'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import {
  createWorkflow,
  createWorkflowNode,
  workflowNodeSize
} from '../../features/workflows/workflowAuthoring'
import { normalizeWorkflowDepartments } from '../../features/workflows/workflowDepartments'
import {
  createWorkflowHistory,
  workflowHistoryReducer
} from '../../features/workflows/workflowHistory'
import { workflowText } from '../../features/workflows/workflowText'
import '../../styles/global.css'

vi.mock('../../features/auth/AccountAuthContext', () => ({ useAccountAuth: () => null }))
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ models: [], enabledModels: [] })
}))
vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'zh-CN', t: (key: string) => key })
}))
const { WorkflowGraphEditor } = await import('../../features/workflows/WorkflowGraphEditor')

let current: WorkflowDefinition
function Harness({ initial }: { initial: WorkflowDefinition }) {
  const [history, dispatch] = useReducer(workflowHistoryReducer, initial, createWorkflowHistory)
  useEffect(() => {
    current = history.present!
  }, [history.present])
  return (
    <div style={{ height: 860, display: 'flex', flexDirection: 'column' }}>
      <div style={{ display: 'flex', gap: 12, padding: 12 }}>
        <button disabled={!history.past.length} onClick={() => dispatch({ type: 'undo' })}>
          撤销
        </button>
        <button disabled={!history.future.length} onClick={() => dispatch({ type: 'redo' })}>
          重做
        </button>
      </div>
      <div style={{ flex: 1, minHeight: 0 }}>
        <WorkflowGraphEditor
          definition={history.present!}
          text={workflowText('zh-CN')}
          onChange={(update, options) =>
            dispatch({ type: 'change', update, options, at: Date.now() })
          }
        />
      </div>
    </div>
  )
}

function fixture(): WorkflowDefinition {
  return normalizeWorkflowDepartments({
    ...createWorkflow(),
    name: '研发组织',
    departments: [
      { id: 'research', name: '研究部', parentId: null, x: 40, y: 60, width: 700, height: 490 },
      {
        id: 'methods',
        name: '方法组',
        parentId: 'research',
        x: 370,
        y: 270,
        width: 310,
        height: 180
      },
      { id: 'review', name: '评审部', parentId: null, x: 790, y: 90, width: 310, height: 260 }
    ],
    nodes: [
      {
        ...createWorkflowNode('研究主管', 100, 140),
        rank: 8,
        managementRole: 'department_admin',
        task: '协调研究与方法提取',
        receives: '研究问题',
        delivers: '方法报告'
      },
      { ...createWorkflowNode('研究员', 120, 210), rank: 3 },
      { ...createWorkflowNode('方法提取', 400, 320), rank: 2 },
      { ...createWorkflowNode('方法验证', 415, 330), rank: 2 },
      { ...createWorkflowNode('评审主管', 825, 160), rank: 6, managementRole: 'department_admin' },
      { ...createWorkflowNode('评审员', 840, 170), rank: 2 },
      {
        ...createWorkflowNode('组织协调员', 800, 580),
        rank: 9,
        managementRole: 'organization_admin'
      }
    ]
  })
}

type Rectangle = { x: number; y: number; width: number; height: number }
function contains(outer: Rectangle, inner: Rectangle) {
  return (
    outer.x <= inner.x &&
    outer.y <= inner.y &&
    outer.x + outer.width >= inner.x + inner.width &&
    outer.y + outer.height >= inner.y + inner.height
  )
}
function overlaps(first: Rectangle, second: Rectangle) {
  return (
    first.x < second.x + second.width &&
    first.x + first.width > second.x &&
    first.y < second.y + second.height &&
    first.y + first.height > second.y
  )
}
function geometry(graph: WorkflowDefinition) {
  return {
    nodes: graph.nodes.map(({ id, x, y }) => ({ id, x, y })),
    departments: graph.departments?.map(({ id, x, y, width, height }) => ({
      id,
      x,
      y,
      width,
      height
    }))
  }
}
function content(graph: WorkflowDefinition) {
  return graph.nodes.map((node) =>
    Object.fromEntries(Object.entries(node).filter(([field]) => field !== 'x' && field !== 'y'))
  )
}
function expectSeparatedMembers(graph: WorkflowDefinition) {
  const nodes = graph.nodes.map((node) => ({ ...node, ...workflowNodeSize(node) }))
  for (let index = 0; index < nodes.length; index++)
    for (const other of nodes.slice(index + 1)) expect(overlaps(nodes[index], other)).toBe(false)
  for (const department of graph.departments ?? []) {
    for (const child of graph.departments ?? []) {
      if (child.parentId === department.id) expect(contains(department, child)).toBe(true)
      else if (child.id !== department.id && child.parentId === department.parentId)
        expect(overlaps(department, child)).toBe(false)
    }
    for (const node of nodes) {
      if (node.departmentId === department.id) expect(contains(department, node)).toBe(true)
    }
  }
  // Geometry must encode the same ownership as the stored department tree.
  expect(content(normalizeWorkflowDepartments(graph))).toEqual(content(graph))
}

beforeEach(async () => {
  await page.viewport(1440, 900)
  for (const [key, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(key, value)
})

describe('organization template automatic layout', () => {
  it('untangles nested departments in one reversible edit while preserving the selected member', async () => {
    const initial = fixture()
    await render(<Harness initial={initial} />)
    await userEvent.dblClick(page.getByRole('group', { name: '节点 研究主管', exact: true }))
    await page.getByRole('tab', { name: '职级', exact: true }).click()
    await expect.element(page.getByRole('spinbutton', { name: '职级', exact: true })).toHaveValue(8)
    await expect.element(page.getByRole('button', { name: '撤销', exact: true })).toBeDisabled()
    const optimize = page.getByRole('button', { name: '优化布局', exact: true })
    await expect.element(optimize).toBeEnabled()
    await optimize.click()
    await expect.poll(() => geometry(current)).not.toEqual(geometry(initial))
    expect(content(current)).toEqual(content(initial))
    expectSeparatedMembers(current)
    const optimized = structuredClone(current)
    await expect
      .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
      .toHaveValue('研究主管')
    await expect.element(page.getByRole('spinbutton', { name: '职级', exact: true })).toHaveValue(8)
    await expect.element(page.getByText('所属部门', { exact: true })).not.toBeInTheDocument()

    await optimize.click()
    expect(geometry(current)).toEqual(geometry(optimized))
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    expect(geometry(current)).toEqual(geometry(initial))
    await expect.element(page.getByRole('button', { name: '撤销', exact: true })).toBeDisabled()
    await page.getByRole('button', { name: '重做', exact: true }).click()
    expect(geometry(current)).toEqual(geometry(optimized))
    await expect
      .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
      .toHaveValue('研究主管')
    await page.getByRole('tab', { name: '任务', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
      .toHaveValue('协调研究与方法提取')
    await page.getByRole('button', { name: '关闭配置面板', exact: true }).click()
    await page.getByRole('button', { name: '适应画布', exact: true }).click()
    await page.screenshot({
      path: '../../../../../.cache/organization-template/auto-layout-light.png'
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    await page.screenshot({
      path: '../../../../../.cache/organization-template/auto-layout-dark.png'
    })
  })

  it('arranges empty department trees even before any members have been added', async () => {
    const initial = { ...fixture(), nodes: [] }
    await render(<Harness initial={initial} />)
    const optimize = page.getByRole('button', { name: '优化布局', exact: true })
    await expect.element(optimize).toBeEnabled()
    await optimize.click()
    await expect.poll(() => geometry(current)).not.toEqual(geometry(initial))
    expect(current.nodes).toHaveLength(0)
    expect(current.departments?.map(({ id, name, parentId }) => ({ id, name, parentId }))).toEqual(
      initial.departments?.map(({ id, name, parentId }) => ({ id, name, parentId }))
    )
    expectSeparatedMembers(current)
    await page.getByRole('button', { name: '撤销', exact: true }).click()
    expect(geometry(current)).toEqual(geometry(initial))
  })
})
