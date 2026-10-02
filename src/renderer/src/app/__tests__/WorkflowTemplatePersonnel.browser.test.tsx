import { page, userEvent } from 'vitest/browser'
import { useEffect, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowDefinition } from '@mycopilot/protocol'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { classicDarkTheme } from '../../config/themes/classic'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'
import { normalizeWorkflowDepartments } from '../../features/workflows/workflowDepartments'
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
  const [graph, setGraph] = useState(initial)
  useEffect(() => {
    current = graph
  }, [graph])
  return (
    <div style={{ width: '100%', height: 740 }}>
      <WorkflowGraphEditor definition={graph} text={workflowText('zh-CN')} onChange={setGraph} />
    </div>
  )
}
function definition() {
  return {
    ...createWorkflow(),
    nodes: [
      {
        ...createWorkflowNode('研究员', 160, 160),
        receives: '原始材料',
        task: '研究问题',
        delivers: '研究结果'
      }
    ]
  }
}
beforeEach(async () => {
  await page.viewport(1280, 820)
  for (const [key, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(key, value)
})

describe('organization template personnel configuration', () => {
  it('adds a rank tab while retaining the existing task fields and top-row settings', async () => {
    const graph = definition()
    await render(<Harness initial={graph} />)
    expect(document.querySelector('.workflow-node__rank')).toHaveTextContent('1')
    expect(document.querySelector('.workflow-node__rank.is-admin')).toBeNull()
    expect(document.querySelector('.workflow-node__mailbox')).toBeNull()
    await userEvent.dblClick(page.getByRole('group', { name: '节点 研究员', exact: true }))
    await expect
      .element(page.getByRole('textbox', { name: '节点名称', exact: true }))
      .toHaveValue('研究员')
    await expect
      .element(page.getByRole('textbox', { name: '这个节点会收到什么', exact: true }))
      .toHaveValue('原始材料')
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
      .toHaveValue('研究问题')
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要交付什么', exact: true }))
      .toHaveValue('研究结果')
    const topbar = document.querySelector('.workflow-graph-editor__topbar')!
    const modelPicker = topbar.querySelector('.workflow-node-model__picker')
    await page.getByRole('tab', { name: '职级', exact: true }).click()
    await page.getByRole('spinbutton', { name: '职级', exact: true }).fill('8')
    await page.getByRole('button', { name: '管理身份', exact: true }).click()
    await expect
      .element(page.getByRole('option', { name: '部门管理员', exact: true }))
      .toBeDisabled()
    await page.getByRole('option', { name: '组织管理员', exact: true }).click()
    expect(current.nodes[0]).toMatchObject({ rank: 8, managementRole: 'organization_admin' })
    expect(document.querySelector('.workflow-node__rank.is-admin')).toHaveTextContent('8')
    expect(topbar.querySelector('.workflow-node-model__picker')).toBe(modelPicker)
    await expect.element(page.getByText('所属部门', { exact: true })).not.toBeInTheDocument()
    await page.screenshot({ path: '../../../../../.cache/organization-templates/rank-light.png' })
    await page.getByRole('tab', { name: '任务', exact: true }).click()
    await expect
      .element(page.getByRole('textbox', { name: '这个节点需要做什么', exact: true }))
      .toHaveValue('研究问题')
  })

  it('assigns department administration to the contained department without duplicating membership details', async () => {
    const graph = normalizeWorkflowDepartments({
      ...definition(),
      departments: [
        { id: 'parent', name: '研究部', parentId: null, x: 40, y: 70, width: 650, height: 410 },
        { id: 'child', name: '方法组', parentId: 'parent', x: 110, y: 120, width: 400, height: 240 }
      ]
    })
    for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, classicDarkTheme)))
      document.documentElement.style.setProperty(key, value)
    await render(<Harness initial={graph} />)
    await userEvent.dblClick(page.getByRole('group', { name: '节点 研究员', exact: true }))
    await page.getByRole('tab', { name: '职级', exact: true }).click()
    await expect.element(page.getByText('所属部门', { exact: true })).not.toBeInTheDocument()
    await expect.element(page.getByText('研究部 / 方法组', { exact: true })).not.toBeInTheDocument()
    await page.getByRole('button', { name: '管理身份', exact: true }).click()
    await page.getByRole('option', { name: '部门管理员', exact: true }).click()
    expect(current.nodes[0]).toMatchObject({
      departmentId: 'child',
      managementRole: 'department_admin'
    })
    expect(document.querySelector('.workflow-node__rank.is-admin')).toHaveTextContent('1')
    expect(document.querySelector('select[aria-label="所属部门"]')).toBeNull()
    await expect
      .element(page.getByText('可以管理本部门及下级部门内职级低于自己的成员。', { exact: true }))
      .toBeVisible()
    await page.screenshot({ path: '../../../../../.cache/organization-templates/rank-dark.png' })
  })
})
