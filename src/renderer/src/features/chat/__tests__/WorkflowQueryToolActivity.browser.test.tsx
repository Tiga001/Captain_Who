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
import { WorkflowNavigationProvider } from '../../workflows/WorkflowNavigationContext'
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

  it('uses one active inbox carousel, separate whole-mailbox counts and current-message actions', async () => {
    const open = vi.fn()
    const openWorkflow = vi.fn()
    const view = await render(
      <WorkflowNavigationProvider onOpenWorkflow={openWorkflow}>
        <ConversationNavigationProvider onOpenConversation={open}>
          <WorkflowMailboxToolActivity
            call={mailCall}
            result={mailResult(
              [
                { ...message, status: 'pending' },
                {
                  ...message,
                  messageId: 'second',
                  sourceNodeName: '分发',
                  sourceConversationId: 'dispatch-chat',
                  content: '第二封正在处理的邮件',
                  status: 'processing'
                }
              ],
              {
                view: 'overview',
                observedAt: 1790942400000,
                counts: {
                  total: 19,
                  pending: 7,
                  processing: 2,
                  processed: 6,
                  stopped: 2,
                  failed: 1,
                  recalled: 1
                },
                history: {
                  total: 10,
                  messages: [{ ...message, content: '历史正文不应显示' }],
                  nextCursor: 'history-next'
                },
                nextCursor: 3
              }
            )}
          />
        </ConversationNavigationProvider>
      </WorkflowNavigationProvider>
    )
    const label =
      '已查看组织收件箱 · 共 19 封邮件 · 待处理 7 · 处理中 2 · 已处理 6 · 已停止 2 · 失败 1 · 已撤回 1'
    await view.getByText(label).click()
    expect(view.container.querySelector('.workflow-query__header')).toBeNull()
    for (const omitted of ['文案润色', '查询快照', '打开组织', 'private-workflow'])
      expect(view.container.textContent).not.toContain(omitted)
    expect(view.container.querySelector('.workflow-send-message time')?.textContent).toBe(
      new Date(message.createdAt).toLocaleString('zh-CN', {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      })
    )
    expect(view.container.textContent).not.toContain('本次返回 2 封待处理或处理中的邮件')
    expect(view.container.querySelector('.workflow-mail-carousel__heading')).toBeNull()
    const inboxActions = view.container.querySelector('.workflow-send-message__actions')!
    const inboxNavigation = inboxActions.querySelector('nav')!
    const inboxJump = inboxActions.querySelector('button[aria-label="打开对话 · 文书"]')!
    expect(inboxNavigation).not.toBeNull()
    expect(
      inboxNavigation.compareDocumentPosition(inboxJump) & Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0)
    await expect.element(view.getByText('还有未返回的待处理或处理中的邮件。')).toBeVisible()
    await expect.element(view.getByText('1 / 2')).toBeVisible()
    await expect.element(view.getByRole('button', { name: '上一封' })).toBeDisabled()
    expect(view.container.querySelectorAll('.workflow-send-message')).toHaveLength(1)
    expect(view.container.textContent).not.toContain('第二封正在处理的邮件')
    await view.getByRole('button', { name: '复制邮件 · 文书', exact: true }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(message.content)
    await view.getByRole('button', { name: '打开对话 · 文书', exact: true }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('writer-chat')
    expect(openWorkflow).not.toHaveBeenCalled()
    await view.getByRole('button', { name: '下一封' }).click()
    await expect.element(view.getByText('2 / 2')).toBeVisible()
    await expect.element(view.getByText('处理中', { exact: true })).toBeVisible()
    await expect.element(view.getByRole('button', { name: '下一封' })).toBeDisabled()
    expect(view.container.querySelectorAll('.workflow-send-message')).toHaveLength(1)
    expect(view.container.textContent).not.toContain(message.content)
    await view.getByRole('button', { name: '复制邮件 · 分发', exact: true }).click()
    expect(copy).toHaveBeenLastCalledWith('第二封正在处理的邮件')
    await view.getByRole('button', { name: '打开对话 · 分发', exact: true }).click()
    expect(open).toHaveBeenLastCalledWith('dispatch-chat')
    await view.getByText(label).click()
    await view.getByText(label).click()
    await expect.element(view.getByText('2 / 2')).toBeVisible()
    for (const forbidden of ['private-batch', '任务已完成', '技术详情', '历史正文不应显示'])
      expect(view.container.textContent).not.toContain(forbidden)
    // Switching from a populated result to an empty one cannot leave stale expanded details.
    await view.rerender(<WorkflowMailboxToolActivity call={mailCall} result={mailResult([])} />)
    expect(view.container.querySelector('details')).toBeNull()
  })

  it('summarizes historical statuses without exposing history indices or making a history-only inbox expandable', async () => {
    const view = await render(
      <WorkflowMailboxToolActivity
        call={mailCall}
        result={mailResult([], {
          view: 'overview',
          counts: {
            total: 4,
            pending: 0,
            processing: 0,
            processed: 1,
            stopped: 1,
            failed: 1,
            recalled: 1
          },
          history: {
            total: 4,
            messages: [{ ...message, sourceNodeName: '历史发送人', content: '私有历史正文' }],
            nextCursor: 'more-history'
          }
        })}
      />
    )
    await expect
      .element(
        view.getByText('已查看组织收件箱 · 共 4 封邮件 · 已处理 1 · 已停止 1 · 失败 1 · 已撤回 1')
      )
      .toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    expect(view.container.textContent).not.toContain('历史发送人')
    expect(view.container.textContent).not.toContain('私有历史正文')
    expect(view.container.textContent).not.toContain('暂无邮件')
    // Stored receipts from the old full-history response obey the same display rule.
    await view.rerender(
      <WorkflowMailboxToolActivity call={mailCall} result={mailResult([message])} />
    )
    await expect
      .element(view.getByText('已查看组织收件箱 · 本次查到 1 封邮件 · 已停止 1'))
      .toBeVisible()
    expect(view.container.querySelector('details')).toBeNull()
    expect(view.container.textContent).not.toContain(message.content)
    await view.rerender(
      <WorkflowMailboxToolActivity
        call={mailCall}
        result={mailResult([
          message,
          { ...message, messageId: 'active', status: 'pending', content: '本次唯一待处理邮件' }
        ])}
      />
    )
    await view.getByText('已查看组织收件箱 · 本次查到 2 封邮件 · 已停止 1').click()
    await expect.element(view.getByText('本次唯一待处理邮件')).toBeVisible()
    expect(view.container.textContent).not.toContain(message.content)
    expect(view.container.querySelector('nav')).toBeNull()
  })

  it('allows one explicitly queried historical message, including an old receipt without a view field', async () => {
    const preciseCall = { ...mailCall, args: { direction: 'inbox', messageId: message.messageId } }
    const view = await render(
      <WorkflowMailboxToolActivity call={preciseCall} result={mailResult([message])} />
    )
    await view.getByText('已查看组织收件箱 · 本次查到 1 封邮件').click()
    await expect.element(view.getByText(message.content)).toBeVisible()
    await expect.element(view.getByText('已停止', { exact: true })).toBeVisible()
    expect(view.container.querySelector('nav')).toBeNull()
    await view.getByRole('button', { name: '复制邮件 · 文书' }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(message.content)
    await view.rerender(
      <WorkflowMailboxToolActivity
        call={preciseCall}
        result={mailResult(
          [{ ...message, messageId: 'different', content: '不得展示其他历史邮件' }, message],
          { view: 'message' }
        )}
      />
    )
    expect(view.container.querySelectorAll('.workflow-send-message')).toHaveLength(1)
    expect(view.container.textContent).not.toContain('不得展示其他历史邮件')
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

  it('cycles English outbox history and resets long-body expansion and copied state between messages', async () => {
    config.language = 'en'
    const content = '这是正文。'.repeat(150)
    const open = vi.fn()
    const openWorkflow = vi.fn()
    const view = await render(
      <WorkflowNavigationProvider onOpenWorkflow={openWorkflow}>
        <ConversationNavigationProvider onOpenConversation={open}>
          <WorkflowMailboxToolActivity
            call={mailCall}
            result={mailResult(
              [
                { ...message, content },
                {
                  ...message,
                  messageId: 'second',
                  content: 'Next letter',
                  status: 'processed',
                  targetNodeName: 'Publisher',
                  targetConversationId: 'publisher-chat'
                }
              ],
              { direction: 'outbox', observedAt: 1790942400000 }
            )}
          />
        </ConversationNavigationProvider>
      </WorkflowNavigationProvider>
    )
    await view.getByText('Checked organization outbox · 2 messages in this result').click()
    expect(view.container.querySelector('.workflow-mail-carousel__heading')).toBeNull()
    const outboxActions = view.container.querySelector('.workflow-send-message__actions')!
    const outboxNavigation = outboxActions.querySelector('nav')!
    const outboxJump = outboxActions.querySelector('button[aria-label="Open conversation · 审核"]')!
    expect(outboxNavigation).not.toBeNull()
    expect(
      outboxNavigation.compareDocumentPosition(outboxJump) & Node.DOCUMENT_POSITION_FOLLOWING
    ).not.toBe(0)
    expect(view.container.querySelector('.workflow-query__header')).toBeNull()
    for (const omitted of ['文案润色', 'Snapshot', 'Open organization', 'private-workflow'])
      expect(view.container.textContent).not.toContain(omitted)
    expect(view.container.querySelector('.workflow-send-message time')?.textContent).toBe(
      new Date(message.createdAt).toLocaleString('en', {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      })
    )
    expect(view.container.querySelector('.workflow-send-message__body')?.textContent).not.toBe(
      content
    )
    await view.getByRole('button', { name: 'Read full message' }).click()
    expect(view.container.querySelector('.workflow-send-message__body')?.textContent).toBe(content)
    await view.getByRole('button', { name: 'Open conversation · 审核' }).click()
    expect(open).toHaveBeenCalledExactlyOnceWith('review-chat')
    expect(openWorkflow).not.toHaveBeenCalled()
    await view.getByRole('button', { name: 'Copy message · 审核' }).click()
    expect(copy).toHaveBeenCalledExactlyOnceWith(content)
    await view.getByRole('button', { name: 'Next message' }).click()
    await expect.element(view.getByText('Next letter')).toBeVisible()
    expect(view.container.querySelectorAll('.workflow-send-message')).toHaveLength(1)
    await expect
      .element(view.getByRole('button', { name: 'Copy message · Publisher' }))
      .toBeVisible()
    await view.getByRole('button', { name: 'Open conversation · Publisher' }).click()
    expect(open).toHaveBeenLastCalledWith('publisher-chat')
    await view.getByRole('button', { name: 'Previous message' }).click()
    await expect.element(view.getByRole('button', { name: 'Read full message' })).toBeVisible()
    await expect.element(view.getByRole('button', { name: 'Copy message · 审核' })).toBeVisible()
    expect(view.container.querySelector('.workflow-send-message__body')?.textContent).not.toBe(
      content
    )
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
          <ConversationNavigationProvider onOpenConversation={vi.fn()}>
            <WorkflowMailboxToolActivity
              call={mailCall}
              result={mailResult(
                [
                  {
                    ...message,
                    status: 'pending',
                    sourceNodeName: '名称很长的组织成员用于检查窄栏里的邮件导航是否被挤压'
                  },
                  { ...message, messageId: 'next', status: 'processing' }
                ],
                {
                  view: 'overview',
                  counts: {
                    total: 104,
                    pending: 70,
                    processing: 9,
                    processed: 20,
                    stopped: 2,
                    failed: 1,
                    recalled: 2
                  },
                  nextCursor: 'next-page'
                }
              )}
            />
          </ConversationNavigationProvider>
        </div>
      )
      await view
        .getByText(
          '已查看组织收件箱 · 共 104 封邮件 · 待处理 70 · 处理中 9 · 已处理 20 · 已停止 2 · 失败 1 · 已撤回 2'
        )
        .click()
      const panel = view.container.firstElementChild as HTMLElement
      expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth)
      const panelBounds = panel.getBoundingClientRect()
      for (const item of panel.querySelectorAll(
        '.workflow-send-message, .workflow-mail-carousel__navigation'
      )) {
        const bounds = item.getBoundingClientRect()
        expect(bounds.left).toBeGreaterThanOrEqual(panelBounds.left + 16)
        expect(bounds.right).toBeLessThanOrEqual(panelBounds.right - 16)
      }
      expect(panel.querySelectorAll('.workflow-send-message')).toHaveLength(1)
      expect(panel.querySelector('.workflow-mail-carousel__heading')).toBeNull()
      const header = panel.querySelector('.workflow-send-message__header')!
      const actions = header.querySelector('.workflow-send-message__actions')!
      const navigation = actions.querySelector('nav')!
      const jump = actions.querySelector('button[aria-label^="打开对话"]')!
      const copyButton = actions.querySelector('button[aria-label^="复制邮件"]')!
      expect(navigation).not.toBeNull()
      expect(jump).not.toBeNull()
      expect(copyButton).not.toBeNull()
      const headerBounds = header.getBoundingClientRect()
      const navigationBounds = navigation.getBoundingClientRect()
      for (const control of [navigation, jump, copyButton]) {
        const bounds = control.getBoundingClientRect()
        expect(bounds.left).toBeGreaterThanOrEqual(headerBounds.left + 11)
        expect(bounds.right).toBeLessThanOrEqual(headerBounds.right - 11)
        expect(
          Math.abs(
            (bounds.top + bounds.bottom) / 2 - (navigationBounds.top + navigationBounds.bottom) / 2
          )
        ).toBeLessThanOrEqual(1)
      }
      expect(navigationBounds.right).toBeLessThanOrEqual(jump.getBoundingClientRect().left)
      expect(jump.getBoundingClientRect().right).toBeLessThanOrEqual(
        copyButton.getBoundingClientRect().left
      )
      await page.screenshot({
        element: panel,
        path: `../../../../../../.cache/workflow-authoring/query-compact-${name}.png`
      })
    })
  }
})
