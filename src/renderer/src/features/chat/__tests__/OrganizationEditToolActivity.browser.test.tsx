import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { OrganizationEditToolActivity } from '../components/toolActivities/OrganizationEditToolActivity'
import { AgentToolActivity } from '../components/toolActivities/AgentToolActivity'
import type { ChatAgentRunView } from '../chatTypes'

const config = vi.hoisted(() => ({ language: 'zh-CN' }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ ...config, t: (key: string) => key })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
const call: AgentToolCall = {
  id: 'private-call',
  tool: 'organization_edit',
  args: {
    reason: '完善评审职责',
    changes: [{ action: 'update_member', nodeId: 'private-node', task: '检查交付结果' }]
  },
  approvalStatus: 'not_required',
  reason: null
}
const member = {
  action: 'update_member',
  entityType: 'member',
  entityId: 'private-node',
  entityName: '评审员',
  conversationId: 'private-chat',
  fields: [
    { field: 'name', before: '临时评审', after: '评审员' },
    { field: 'task', before: '检查界面', after: '检查交付结果\n核对验收清单' },
    { field: 'receives', before: '', after: '设计稿与实现说明' },
    { field: 'delivers', before: '意见', after: '验收结论' },
    { field: 'rank', before: 1, after: 3 },
    { field: 'managementRole', before: 'member', after: 'department_admin' },
    { field: 'departmentId', before: null, after: 'private-department', afterLabel: '质量部' },
    {
      field: 'modelConfigId',
      before: 'private-model-a',
      after: 'private-model-b',
      beforeLabel: 'Fast model',
      afterLabel: 'Review model'
    },
    { field: 'permissionMode', before: 'default', after: 'custom' }
  ]
}
const department = {
  action: 'update_department',
  entityType: 'department',
  entityId: 'private-department',
  entityName: '质量部',
  fields: [
    { field: 'name', before: '测试组', after: '质量部' },
    { field: 'parentId', before: null, after: 'private-parent', afterLabel: '产品部' }
  ]
}
function receipt(changes: unknown[] = [member, department]): AgentToolResult {
  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: {
      instanceId: 'private-instance',
      organizationName: '产品组织',
      organizationRevision: 4,
      affectedConversationIds: ['private-chat'],
      changes
    }
  }
}
afterEach(() => {
  config.language = 'zh-CN'
})

describe('organization edit presentation', () => {
  it('routes the unified tool into concise member and department field diffs', async () => {
    const view = await render(
      <AgentToolActivity
        call={call}
        result={receipt()}
        run={{} as ChatAgentRunView}
        showImageGenerationPreview={false}
      />
    )
    const label = '组织变动【产品组织】 · 完善评审职责'
    await expect.element(view.getByText(label)).toBeVisible()
    await expect.element(view.getByText('更新成员 · 评审员')).not.toBeVisible()
    await view.getByText(label).click()
    for (const value of [
      '更新成员 · 评审员',
      '临时评审',
      '检查交付结果\n核对验收清单',
      '设计稿与实现说明',
      '验收结论',
      '普通成员',
      '部门管理员',
      'Fast model',
      'Review model',
      '默认权限',
      '自定义权限',
      '更新部门 · 质量部',
      '产品部'
    ])
      await expect.element(view.getByText(value, { exact: true })).toBeVisible()
    expect(view.container.querySelectorAll('.organization-edit-change')).toHaveLength(2)
    expect(view.getByRole('navigation').elements()).toHaveLength(0)
    expect(
      view.container.querySelector('.organization-edit-change__heading .lucide-building-2')
    ).not.toBeNull()
    for (const hidden of [
      'private-',
      'organization_edit',
      'organizationRevision',
      'department_admin',
      'modelConfigId',
      '参数',
      '技术详情'
    ])
      expect(view.container.textContent).not.toContain(hidden)
  })

  it('pages member and department changes independently in receipt order', async () => {
    const view = await render(
      <OrganizationEditToolActivity
        call={call}
        result={receipt([
          { action: 'add_member', entityType: 'member', entityName: '研究员', fields: [] },
          { action: 'add_department', entityType: 'department', entityName: '研究部', fields: [] },
          { action: 'remove_member', entityType: 'member', entityName: '临时成员', fields: [] },
          {
            action: 'remove_department',
            entityType: 'department',
            entityName: '旧部门',
            fields: []
          }
        ])}
      />
    )
    await view.getByText('组织变动【产品组织】 · 完善评审职责').click()
    const members = view.getByRole('navigation', { name: '切换成员变更' })
    const departments = view.getByRole('navigation', { name: '切换部门变更' })
    await expect.element(members.getByText('1 / 2')).toBeVisible()
    await expect.element(departments.getByText('1 / 2')).toBeVisible()
    await expect.element(view.getByText('添加成员 · 研究员')).toBeVisible()
    await expect.element(view.getByText('添加部门 · 研究部')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '上一位', exact: true })).toBeDisabled()
    await expect.element(view.getByRole('button', { name: '上一项', exact: true })).toBeDisabled()
    expect(view.container.querySelectorAll('.organization-edit-change')).toHaveLength(2)
    expect(view.container.textContent).not.toContain('移除成员 · 临时成员')
    expect(view.container.textContent).not.toContain('移除部门 · 旧部门')

    await view.getByRole('button', { name: '下一位', exact: true }).click()
    await expect.element(view.getByText('移除成员 · 临时成员')).toBeVisible()
    await expect.element(members.getByText('2 / 2')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '下一位', exact: true })).toBeDisabled()
    await expect.element(view.getByText('添加部门 · 研究部')).toBeVisible()
    await expect.element(departments.getByText('1 / 2')).toBeVisible()

    await view.getByRole('button', { name: '下一项', exact: true }).click()
    await expect.element(view.getByText('移除部门 · 旧部门')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '下一项', exact: true })).toBeDisabled()
    await expect.element(view.getByText('移除成员 · 临时成员')).toBeVisible()
    await view.getByRole('button', { name: '上一位', exact: true }).click()
    await expect.element(view.getByText('添加成员 · 研究员')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '上一位', exact: true })).toBeDisabled()
    await expect.element(view.getByText('移除部门 · 旧部门')).toBeVisible()
  })

  it('retains each page across disclosure and receipt refreshes, clamps removal and resets for a new call', async () => {
    const secondMember = { ...member, entityId: 'second-member', entityName: '研究员', fields: [] }
    const thirdMember = { ...member, entityId: 'third-member', entityName: '设计师', fields: [] }
    const secondDepartment = {
      ...department,
      entityId: 'second-department',
      entityName: '研究部',
      fields: []
    }
    const changes = [member, department, secondMember, secondDepartment, thirdMember]
    const view = await render(
      <OrganizationEditToolActivity call={call} result={receipt(changes)} />
    )
    const label = '组织变动【产品组织】 · 完善评审职责'
    await view.getByText(label).click()
    await view.getByRole('button', { name: '下一位', exact: true }).click()
    await view.getByRole('button', { name: '下一位', exact: true }).click()
    await view.getByRole('button', { name: '下一项', exact: true }).click()
    await view.getByText(label).click()
    await expect.element(view.getByText('更新成员 · 设计师')).not.toBeVisible()
    await view.getByText(label).click()
    await expect.element(view.getByText('更新成员 · 设计师')).toBeVisible()
    await expect.element(view.getByText('更新部门 · 研究部')).toBeVisible()

    await view.rerender(
      <OrganizationEditToolActivity call={{ ...call }} result={receipt([...changes])} />
    )
    await expect.element(view.getByText('更新成员 · 设计师')).toBeVisible()
    await expect.element(view.getByText('更新部门 · 研究部')).toBeVisible()

    await view.rerender(
      <OrganizationEditToolActivity
        call={call}
        result={receipt([member, department, secondMember, secondDepartment])}
      />
    )
    await expect.element(view.getByText('更新成员 · 研究员')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '下一位', exact: true })).toBeDisabled()
    await expect.element(view.getByText('更新部门 · 研究部')).toBeVisible()

    const nextCall = { ...call, id: 'next-call' }
    await view.rerender(
      <OrganizationEditToolActivity
        call={nextCall}
        result={{ ...receipt(changes), callId: nextCall.id }}
      />
    )
    if (!view.container.querySelector('details')?.open) await view.getByText(label).click()
    await expect.element(view.getByText('更新成员 · 评审员')).toBeVisible()
    await expect.element(view.getByText('更新部门 · 质量部')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '上一位', exact: true })).toBeDisabled()
    await expect.element(view.getByRole('button', { name: '上一项', exact: true })).toBeDisabled()
  })

  it('omits absent groups and navigation for single changes', async () => {
    const view = await render(
      <OrganizationEditToolActivity call={call} result={receipt([member])} />
    )
    await view.getByText('组织变动【产品组织】 · 完善评审职责').click()
    await expect.element(view.getByText('更新成员 · 评审员')).toBeVisible()
    expect(view.container.querySelectorAll('.organization-edit-change')).toHaveLength(1)
    expect(
      view.container.querySelector('.organization-edit-change__heading .lucide-building-2')
    ).toBeNull()
    expect(view.getByRole('navigation').elements()).toHaveLength(0)
    expect(view.container.textContent).not.toContain('1 / 1')

    await view.rerender(<OrganizationEditToolActivity call={call} result={receipt([department])} />)
    await expect.element(view.getByText('更新部门 · 质量部')).toBeVisible()
    expect(view.container.querySelectorAll('.organization-edit-change')).toHaveLength(1)
    expect(view.container.textContent).not.toContain('更新成员')
    expect(view.getByRole('navigation').elements()).toHaveLength(0)
    expect(view.container.textContent).not.toContain('1 / 1')
  })

  it('never renders unresolved references, unknown fields or raw JSON', async () => {
    const view = await render(
      <OrganizationEditToolActivity
        call={call}
        result={receipt([
          {
            ...member,
            fields: [
              { field: 'modelConfigId', before: 'private-model-a', after: 'private-model-b' },
              { field: 'departmentId', before: 'private-department', after: null },
              { field: 'task', before: { private: 'json' }, after: '明确任务' },
              { field: 'avatarId', before: 'private-avatar-a', after: 'private-avatar-b' }
            ]
          }
        ])}
      />
    )
    await view.getByText('组织变动【产品组织】 · 完善评审职责').click()
    await expect.element(view.getByText('明确任务', { exact: true })).toBeVisible()
    expect(view.container.textContent).toContain('名称暂不可用')
    expect(view.container.textContent).toContain('直属组织')
    for (const hidden of ['private', 'avatarId', '[object Object]', '{', '}'])
      expect(view.container.textContent).not.toContain(hidden)
  })

  it('does not expand an empty successful edit', async () => {
    const view = await render(<OrganizationEditToolActivity call={call} result={receipt([])} />)
    await expect.element(view.getByText('组织配置未发生变化 · 完善评审职责')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
  })

  it('does not expose failure diagnostics or infer a successful edit from a finished turn', async () => {
    const view = await render(
      <OrganizationEditToolActivity
        call={call}
        result={{ ...receipt(), ok: false, error: 'private-backend-diagnostic' }}
      />
    )
    await expect.element(view.getByText('组织编辑失败 · 完善评审职责')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    expect(view.container.textContent).not.toContain('private')
    await view.rerender(<OrganizationEditToolActivity call={call} settledStatus="completed" />)
    await expect.element(view.getByText('组织编辑结果待确认 · 完善评审职责')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
  })

  it('localizes pending, cancelled and completed edits in English', async () => {
    config.language = 'en-US'
    const englishCall = { ...call, args: { reason: 'Clarify reviewer responsibilities' } }
    const view = await render(<OrganizationEditToolActivity call={englishCall} />)
    await expect
      .element(view.getByText('Editing organization · Clarify reviewer responsibilities'))
      .toBeVisible()
    await view.rerender(<OrganizationEditToolActivity call={englishCall} cancelled />)
    await expect
      .element(view.getByText('Organization edit cancelled · Clarify reviewer responsibilities'))
      .toBeVisible()
    await view.rerender(
      <OrganizationEditToolActivity call={englishCall} result={receipt([member])} cancelled />
    )
    await view
      .getByText('Organization changes [产品组织] · Clarify reviewer responsibilities')
      .click()
    await expect.element(view.getByText('Department administrator', { exact: true })).toBeVisible()
    await expect.element(view.getByText('Custom permissions', { exact: true })).toBeVisible()

    await view.rerender(
      <OrganizationEditToolActivity
        call={englishCall}
        result={receipt([
          member,
          department,
          { ...member, entityName: 'Writer', fields: [] },
          { ...department, entityName: 'Research', fields: [] }
        ])}
      />
    )
    await expect.element(view.getByRole('navigation', { name: 'Member changes' })).toBeVisible()
    await expect.element(view.getByRole('navigation', { name: 'Department changes' })).toBeVisible()
    await expect.element(view.getByRole('button', { name: 'Previous member' })).toBeDisabled()
    await expect.element(view.getByRole('button', { name: 'Previous department' })).toBeDisabled()
    await view.getByRole('button', { name: 'Next member' }).click()
    await expect.element(view.getByText('Updated member · Writer')).toBeVisible()
    await view.getByRole('button', { name: 'Next department' }).click()
    await expect.element(view.getByText('Updated department · Research')).toBeVisible()
    await expect.element(view.getByRole('button', { name: 'Next member' })).toBeDisabled()
    await expect.element(view.getByRole('button', { name: 'Next department' })).toBeDisabled()
  })

  it('keeps historical member-management receipts readable without exposing an executable alias', async () => {
    const oldCall = { ...call, tool: 'organization_manage_members' }
    const result = {
      ...receipt(),
      tool: oldCall.tool,
      result: {
        organizationName: '产品组织',
        changes: [
          {
            action: 'set_rank',
            nodeId: 'private-node',
            nodeName: '评审员',
            previousRank: 1,
            rank: 3
          }
        ]
      }
    }
    const view = await render(
      <AgentToolActivity
        call={oldCall}
        result={result}
        run={{} as ChatAgentRunView}
        showImageGenerationPreview={false}
      />
    )
    await view.getByText('组织变动【产品组织】 · 完善评审职责').click()
    await expect.element(view.getByText('更新成员 · 评审员')).toBeVisible()
    expect(view.container.textContent).not.toContain('private-node')
    expect(view.container.textContent).not.toContain('organization_manage_members')
  })
})
