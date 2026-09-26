import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { ChatMessage } from '../chatTypes'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { copyTextToClipboard } from '../../../components/clipboard'
import '../../../styles/global.css'
import '../ChatConversationPage.css'

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      showCacheHitRate: false,
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))
vi.mock('../../../components/clipboard', () => ({
  copyTextToClipboard: vi.fn(async () => undefined)
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
const { ChatMessageItem } = await import('../components/ChatMessageItem')

const assembled =
  '[Workflow collaboration message]\nWorkflow: 新功能开发\n\n[Shared workflow background]\n共同背景完整保留\n\n[Your workflow role]\n负责审核\n\n[Incoming messages]\n暗号：升龙拳\n\n[Your task]\n继续核验\n\n[Destinations and delivery method]\nworkflow_send'
function message(contents: (string | undefined)[] = ['暗号：升龙拳']): ChatMessage {
  return {
    id: 'workflow-message',
    role: 'user',
    content: assembled,
    createdAt: 1,
    status: 'sent',
    workflowSource: {
      inputId: 'input-1',
      instanceId: 'workflow-1',
      workflowName: '新功能开发',
      sources: contents.map((content, index) => ({
        nodeId: `node-${index}`,
        nodeName: `节点${index + 1}`,
        conversationId: `chat-${index}`,
        conversationTitle: `对话${index + 1}`,
        ...(content === undefined ? {} : { content })
      }))
    }
  }
}
function Fixture({
  item,
  onUiStateChange = vi.fn()
}: {
  item: ChatMessage
  onUiStateChange?: (id: string, state: ChatMessage['uiState']) => void
}) {
  return (
    <div
      style={{
        width: 820,
        height: 620,
        padding: 32,
        background: 'var(--mc-color-surface-main-panel)'
      }}
    >
      <div
        className="chat-conversation-page__messages"
        style={{ overflow: 'auto', height: '100%' }}
      >
        <ChatMessageItem
          message={item}
          onEditSubmit={vi.fn()}
          onUiStateChange={onUiStateChange}
          showTokenUsageDetails={false}
        />
      </div>
    </div>
  )
}
let previousStyle: string | null
beforeEach(async () => {
  vi.mocked(copyTextToClipboard).mockClear()
  previousStyle = document.documentElement.getAttribute('style')
  for (const [name, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(name, value)
  await page.viewport(1000, 760)
})
afterEach(() => {
  if (previousStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousStyle)
})

describe('received workflow message disclosure', () => {
  it('defaults to actual source content, expands after the star action, and copies the displayed content', async () => {
    const item = message()
    const onUiStateChange = vi.fn()
    const view = await render(<Fixture item={item} onUiStateChange={onUiStateChange} />)
    const article = view.container.querySelector<HTMLElement>('article')!
    const body = article.querySelector('.chat-message__body')!
    expect(body.textContent).toBe('暗号：升龙拳')
    expect(article.querySelector('[data-input-origin="workflow"]')?.textContent).toContain(
      '来自工作流新功能开发'
    )
    expect(body.textContent).not.toContain('共同背景')
    expect(article.querySelector('button[aria-label="编辑消息"]')).toBeNull()
    await userEvent.hover(article)
    const toggle = view.getByRole('button', { name: '展开工作流上下文' })
    const buttons = [...article.querySelectorAll<Element>('.chat-message__actions button')]
    const toggleIndex = buttons.indexOf(toggle.element())
    expect(buttons[toggleIndex - 1].querySelector('svg.lucide-star')).not.toBeNull()
    await expect.element(toggle).toHaveAttribute('aria-expanded', 'false')
    await page.screenshot({
      element: view.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/workflow-authoring/workflow-received-collapsed.png'
    })
    await view.getByRole('button', { name: '复制消息', exact: true }).click()
    expect(copyTextToClipboard).toHaveBeenLastCalledWith('暗号：升龙拳')
    await toggle.click()
    await expect
      .element(view.getByRole('button', { name: '收起工作流上下文' }))
      .toHaveAttribute('aria-expanded', 'true')
    expect(body.textContent).toContain('共同背景完整保留')
    expect(body.textContent).toContain('workflow_send')
    await page.screenshot({
      element: view.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/workflow-authoring/workflow-received-expanded.png'
    })
    await view.getByRole('button', { name: '已复制', exact: true }).click()
    expect(copyTextToClipboard).toHaveBeenLastCalledWith(assembled)
    await view.getByRole('button', { name: '收起工作流上下文' }).click()
    expect(body.textContent).toBe('暗号：升龙拳')
    expect(item.content).toBe(assembled)
    expect(onUiStateChange).not.toHaveBeenCalled()
  })

  it('shows every batch source body separately without repeating the workflow wrapper', async () => {
    const view = await render(<Fixture item={message(['第一条结果\n详细说明', '第二条结果'])} />)
    const article = view.container.querySelector<HTMLElement>('article')!
    const sections = article.querySelectorAll('.workflow-received-content__message')
    expect(sections).toHaveLength(2)
    expect(sections[0].textContent).toContain('节点1第一条结果')
    expect(sections[1].textContent).toBe('节点2第二条结果')
    expect(article.querySelector('.chat-message__body')?.textContent).not.toContain(
      '[Workflow collaboration message]'
    )
    await userEvent.hover(article)
    await view.getByRole('button', { name: '复制消息', exact: true }).click()
    expect(copyTextToClipboard).toHaveBeenLastCalledWith(
      '节点1\n第一条结果\n详细说明\n\n节点2\n第二条结果'
    )
  })

  it('keeps full legacy or partially enriched content visible and does not add a misleading toggle', async () => {
    const view = await render(<Fixture item={message([undefined])} />)
    expect(view.container.querySelector('.chat-message__body')?.textContent).toContain(
      '共同背景完整保留'
    )
    expect(view.getByRole('button', { name: '展开工作流上下文' }).elements()).toHaveLength(0)
    await view.rerender(<Fixture item={message(['已知正文', undefined])} />)
    expect(view.container.querySelector('.chat-message__body')?.textContent).toContain(
      '共同背景完整保留'
    )
    expect(view.getByRole('button', { name: '展开工作流上下文' }).elements()).toHaveLength(0)
    await view.rerender(<Fixture item={{ ...message(), workflowSource: undefined }} />)
    expect(view.container.querySelector('.chat-message__body')?.textContent).toContain(
      '共同背景完整保留'
    )
    expect(view.getByRole('button', { name: '展开工作流上下文' }).elements()).toHaveLength(0)
  })

  it('resets expansion when the component changes to a different message', async () => {
    const view = await render(<Fixture item={message()} />)
    await userEvent.hover(view.container.querySelector<HTMLElement>('article')!)
    await view.getByRole('button', { name: '展开工作流上下文' }).click()
    await view.rerender(<Fixture item={{ ...message(['新的正文']), id: 'another-message' }} />)
    expect(view.container.querySelector('.chat-message__body')?.textContent).toBe('新的正文')
    await expect
      .element(view.getByRole('button', { name: '展开工作流上下文' }))
      .toHaveAttribute('aria-expanded', 'false')
  })
})
