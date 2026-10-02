import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it, vi, afterEach } from 'vitest'
import { render } from 'vitest-browser-react'
import { page } from 'vitest/browser'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicLightTheme, classicDarkTheme } from '../../../config/themes/classic'
import '../../../styles/global.css'
import '../ChatConversationPage.css'
import { ConversationNavigationProvider } from '../ConversationNavigationContext'
import { WorkflowStateToolActivity } from '../components/toolActivities/WorkflowStateToolActivity'
import { WorkflowMailboxToolActivity } from '../components/toolActivities/WorkflowMailboxToolActivity'
import { AgentToolActivity } from '../components/toolActivities/AgentToolActivity'
import type { ChatAgentRunView } from '../chatTypes'

const config = vi.hoisted(() => ({ language: 'zh-CN' }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ ...config, t: (key: string) => key })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
const copy = vi.fn().mockResolvedValue(undefined)
vi.mock('../components/clipboard', () => ({ copyTextToClipboard: (value: string) => copy(value) }))
const call: AgentToolCall = {
  id: 'query',
  tool: 'organization_get_state',
  args: { view: 'all', reason: '确认上游是否已完成交付' },
  approvalStatus: 'not_required',
  reason: null
}
const result: AgentToolResult = {
  callId: call.id,
  tool: call.tool,
  ok: true,
  result: {
    available: true,
    organizationName: '文案润色',
    instanceId: 'private-workflow',
    executionVersion: 'private-version',
    currentNodeId: 'writer',
    observedAt: 1790856000000,
    background: '旧记录中的流程公共背景',
    runtime: { nodes: [{ nodeId: 'writer', nodeName: '文书', state: 'running' }] },
    topology: { nodes: [{ nodeName: '文书', task: '润色文案' }] }
  }
}
const mailCall = {
  ...call,
  tool: 'organization_get_mailbox',
  args: { direction: 'inbox', limit: 50 }
}
function mailResult(messages: unknown[], extra = {}): AgentToolResult {
  return {
    ...result,
    tool: mailCall.tool,
    result: {
      available: true,
      organizationName: '文案润色',
      instanceId: 'private-workflow',
      observedAt: 1790856000000,
      direction: 'inbox',
      messages,
      nextCursor: null,
      ...extra
    }
  }
}
const message = {
  messageId: 'private-message',
  inputId: 'private-batch',
  sourceNodeName: '文书',
  sourceConversationId: 'writer-chat',
  targetNodeName: '审核',
  targetConversationId: 'review-chat',
  content: '暗号：升龙拳\n保留原始内容',
  bodyAvailable: true,
  status: 'stopped',
  createdAt: 1790856000000
}
afterEach(() => {
  copy.mockClear()
  config.language = 'zh-CN'
})

describe('organization query presentation', () => {
  it('renders state as a non-expandable reason without any returned details', async () => {
    const view = await render(
      <AgentToolActivity
        call={call}
        result={result}
        cancelled
        run={{} as ChatAgentRunView}
        showImageGenerationPreview={false}
      />
    )
    await expect.element(view.getByText('已查看组织 · 确认上游是否已完成交付')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    for (const forbidden of [
      'private-version',
      '文案润色',
      '查询快照',
      '润色文案',
      '技术详情',
      '流程关系',
      '旧记录中的流程公共背景'
    ])
      expect(view.container.textContent).not.toContain(forbidden)
    await view.rerender(<WorkflowStateToolActivity call={call} />)
    await expect.element(view.getByText('正在查看组织 · 确认上游是否已完成交付')).toBeVisible()
    await view.rerender(
      <WorkflowStateToolActivity
        call={call}
        result={{ ...result, ok: false, error: 'private-error' }}
      />
    )
    await expect.element(view.getByText('查看组织失败 · 确认上游是否已完成交付')).toBeVisible()
    expect(view.container.textContent).not.toContain('private-error')
  })

  it('shows the reason in English without exposing query details', async () => {
    config.language = 'en'
    const view = await render(
      <WorkflowStateToolActivity
        call={{ ...call, args: { reason: 'Check member progress' } }}
        result={result}
      />
    )
    await expect
      .element(view.getByText('Checked organization · Check member progress'))
      .toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
  })

  it('never expands empty inbox/outbox and uses the same tray with different curved arrows', async () => {
    const view = await render(
      <AgentToolActivity
        call={mailCall}
        result={mailResult([])}
        run={{} as ChatAgentRunView}
        showImageGenerationPreview={false}
      />
    )
    await expect.element(view.getByText('已查看组织收件箱 · 暂无邮件')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    const inboxPaths = [...view.container.querySelectorAll('svg path')].map((path) =>
      path.getAttribute('d')
    )
    await view.rerender(
      <WorkflowMailboxToolActivity
        call={{ ...mailCall, args: { direction: 'outbox' } }}
        result={mailResult([], { direction: 'outbox' })}
      />
    )
    await expect.element(view.getByText('已查看组织发件箱 · 暂无邮件')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    const outboxPaths = [...view.container.querySelectorAll('svg path')].map((path) =>
      path.getAttribute('d')
    )
    expect(outboxPaths.slice(0, 2)).toEqual(inboxPaths.slice(0, 2))
    expect(outboxPaths.slice(2)).not.toEqual(inboxPaths.slice(2))
    expect(view.container.textContent).not.toContain('技术详情')
  })

  it('preserves message copy, history grouping, sender navigation and simple mail state without technical details', async () => {
    const open = vi.fn()
    const view = await render(
      <ConversationNavigationProvider onOpenConversation={open}>
        <WorkflowMailboxToolActivity
          call={mailCall}
          result={mailResult(
            [message, { ...message, messageId: 'second', sourceNodeName: '分发' }],
            { nextCursor: 3 }
          )}
        />
      </ConversationNavigationProvider>
    )
    await view.getByText('已查看组织收件箱 · 本次查到 2 封邮件').click()
    await expect.element(view.getByText('历史邮件')).toBeVisible()
    await expect.element(view.getByText('还有更多记录，本次查询未全部返回。')).toBeVisible()
    await expect.element(view.getByText('已停止').first()).toBeVisible()
    await view.getByRole('button', { name: '复制邮件 · 文书', exact: true }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(message.content)
    await view.getByRole('button', { name: '打开对话 · 文书', exact: true }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('writer-chat')
    for (const forbidden of ['private-batch', '任务已完成', '技术详情'])
      expect(view.container.textContent).not.toContain(forbidden)
    // Switching from a populated result to an empty one cannot leave stale expanded details.
    await view.rerender(<WorkflowMailboxToolActivity call={mailCall} result={mailResult([])} />)
    expect(view.container.querySelector('details')).toBeNull()
  })

  it('does not expose withheld inbox content or offer copying it', async () => {
    const view = await render(
      <WorkflowMailboxToolActivity
        call={mailCall}
        result={mailResult([
          {
            ...message,
            bodyAvailable: false,
            content: 'must-not-render',
            withholdingReason: 'response_body_budget',
            status: 'pending'
          }
        ])}
      />
    )
    await view.getByText('已查看组织收件箱 · 本次查到 1 封邮件').click()
    await expect.element(view.getByText('本次查询内容较多，未包含这封邮件的正文。')).toBeVisible()
    expect(view.container.textContent).not.toContain('must-not-render')
    expect(view.getByRole('button', { name: /复制邮件/ }).elements()).toHaveLength(0)
  })

  it('collapses long outbox bodies and opens recipients rather than senders', async () => {
    const content = '这是正文。'.repeat(150)
    const open = vi.fn()
    const view = await render(
      <ConversationNavigationProvider onOpenConversation={open}>
        <WorkflowMailboxToolActivity
          call={mailCall}
          result={mailResult([{ ...message, content }], { direction: 'outbox' })}
        />
      </ConversationNavigationProvider>
    )
    await view.getByText('已查看组织发件箱 · 本次查到 1 封邮件').click()
    expect(view.container.querySelector('.workflow-send-message__body')?.textContent).not.toBe(
      content
    )
    await view.getByRole('button', { name: '展开全文' }).click()
    expect(view.container.querySelector('.workflow-send-message__body')?.textContent).toBe(content)
    await view.getByRole('button', { name: '打开对话 · 审核' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('review-chat')
    await view.getByRole('button', { name: '复制邮件 · 审核' }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(content)
  })

  it('shows running, cancellation, failure and unavailable states without empty disclosures', async () => {
    const view = await render(<WorkflowMailboxToolActivity call={mailCall} />)
    await expect.element(view.getByText('正在查看组织收件箱')).toBeVisible()
    await view.rerender(<WorkflowMailboxToolActivity call={mailCall} cancelled />)
    await expect.element(view.getByText('已取消查看组织收件箱')).toBeVisible()
    await view.rerender(
      <WorkflowMailboxToolActivity
        call={mailCall}
        result={{ ...mailResult([]), ok: false, error: 'internal-error' }}
      />
    )
    await expect.element(view.getByText('查看组织收件箱失败')).toBeVisible()
    await view.rerender(<WorkflowMailboxToolActivity call={mailCall} settledStatus="completed" />)
    await expect.element(view.getByText('组织收件箱的查询结果待确认')).toBeVisible()
    await view.rerender(
      <WorkflowMailboxToolActivity
        call={mailCall}
        result={{ ...mailResult([]), result: { available: false } }}
      />
    )
    await expect.element(view.getByText('组织信息暂不可用')).toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
  })

  for (const [name, theme] of [
    ['light', classicLightTheme],
    ['dark', classicDarkTheme]
  ] as const) {
    it(`keeps compact query rows readable in a narrow ${name} panel`, async () => {
      await page.viewport(460, 900)
      const view = await render(
        <div
          style={
            {
              ...getFrontendCssVariables(frontendConfig, theme),
              width: 420,
              padding: 16,
              colorScheme: name,
              fontFamily: 'sans-serif',
              background: 'var(--mc-color-surface-main-panel)',
              display: 'grid',
              gap: 16
            } as CSSProperties
          }
        >
          <WorkflowStateToolActivity call={call} result={result} />
          <WorkflowMailboxToolActivity call={mailCall} result={mailResult([])} />
          <WorkflowMailboxToolActivity
            call={mailCall}
            result={mailResult([], { direction: 'outbox' })}
          />
          <WorkflowMailboxToolActivity call={mailCall} result={mailResult([message])} />
        </div>
      )
      await view.getByText('已查看组织收件箱 · 本次查到 1 封邮件').click()
      const panel = view.container.firstElementChild as HTMLElement
      expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth)
      await page.screenshot({
        element: panel,
        path: `../../../../../../.cache/workflow-authoring/query-compact-${name}.png`
      })
    })
  }
})
