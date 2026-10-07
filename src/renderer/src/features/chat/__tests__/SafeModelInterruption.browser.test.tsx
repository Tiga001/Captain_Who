import type { GitTurnDiffSummary } from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import {
  mapConversationFromStorage,
  mapMessageToStorage
} from '../../storage/storageConversationMapping'
import type { ChatMessage } from '../chatTypes'
import { ChatMessageItem } from '../components/ChatMessageItem'
import '../../../styles/global.css'
import '../ChatConversationPage.css'

const translations: Record<string, string> = {
  'agent.interruption.serviceConnectionFailed': '模型服务连接失败',
  'agent.interruption.outputLimitReached': '输出达到上限，回复未完成',
  'agent.interruption.emptyResponse': '模型未返回有效回复',
  'agent.interruption.streamInterrupted': '模型连接中断，回复未完成',
  'agent.interruption.admissionUnconfirmed': '发送结果尚未确认。已尝试核对，未自动重发。'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: string) => translations[key] ?? key
  })
}))

vi.mock('../../storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

vi.mock('../../imageGeneration/artifacts/hostImageArtifactResolver', () => ({
  hostImageArtifactResolver: {
    resolve: async () => ({
      src: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
    })
  }
}))

let previousRootStyle: string | null

beforeEach(() => {
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [key, value] of Object.entries(getFrontendCssVariables())) {
    document.documentElement.style.setProperty(key, value)
  }
})

afterEach(() => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
})

function element(container: ParentNode, selector: string): HTMLElement {
  const node = container.querySelector<HTMLElement>(selector)
  if (!node) throw new Error(`Missing ${selector}`)
  return node
}

function expectCompactInterruption(container: ParentNode) {
  const article = element(container, '.chat-message--assistant')
  const body = element(article, '.chat-message__body')
  const row = element(body, '.agent-run__interruption')
  expect(getComputedStyle(article).padding).toBe('0px')
  expect(getComputedStyle(body).backgroundColor).toBe('rgba(0, 0, 0, 0)')
  expect(getComputedStyle(body).borderTopWidth).toBe('0px')
  expect(getComputedStyle(body).padding).toBe('0px')
  expect(getComputedStyle(row).backgroundColor).toBe('rgba(0, 0, 0, 0)')
  expect(getComputedStyle(row).borderTopWidth).toBe('0px')
  expect(getComputedStyle(row).color).not.toBe(getComputedStyle(article).color)
  expect(getComputedStyle(row).color).not.toBe(getComputedStyle(body).color)
  expect(container.querySelectorAll('.agent-run__interruption')).toHaveLength(1)
}

function reloadMessage(message: ChatMessage): ChatMessage {
  return mapConversationFromStorage({
    id: 'conversation-safe-interruption',
    title: 'Interrupted conversation',
    createdAt: 1,
    updatedAt: 2,
    messages: [mapMessageToStorage(message)]
  }).messages[0]!
}

function interruptedMessage(): ChatMessage {
  return {
    id: 'assistant-safe-interruption',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'sent',
    agentRun: {
      runId: 'run-safe-interruption',
      status: 'failed',
      startedAt: 1,
      completedAt: 2,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: [],
      interruption: { reason: 'service_connection_failed' }
    }
  }
}

describe('safe model interruption status', () => {
  it.each([
    ['service_connection_failed', '模型服务连接失败', ''],
    ['output_limit_reached', '输出达到上限，回复未完成', ''],
    ['empty_response', '模型未返回有效回复', ''],
    ['stream_interrupted', '模型连接中断，回复未完成', '已生成的部分正文']
  ] as const)(
    'shows %s once without a recovery button, including empty replies',
    async (reason, text, content) => {
      const message = interruptedMessage()
      // Durable terminal reconciliation may label a failed run's message as error.
      message.status = 'error'
      message.content = content
      message.agentRun!.interruption = { reason }
      message.agentRun!.finishReason = reason === 'output_limit_reached' ? 'length' : undefined
      message.agentRun!.error = 'raw provider diagnostic must not be duplicated'
      const screen = await render(
        <ChatMessageItem message={message} showTokenUsageDetails={false} />
      )
      await expect.element(screen.getByText(text)).toBeVisible()
      expect(screen.container.querySelectorAll('.agent-run__interruption')).toHaveLength(1)
      expect(screen.container.querySelector('.agent-run__notice')).toBeNull()
      expect(screen.container.querySelector('.agent-run__error')).toBeNull()
      expect(screen.container.querySelector('button')).toBeNull()
      expectCompactInterruption(screen.container)
      if (content) await expect.element(screen.getByText(content)).toBeVisible()
    }
  )

  it('uses the same red interruption row for legacy token-limit finish reasons', async () => {
    const message = interruptedMessage()
    message.status = 'error'
    message.agentRun!.interruption = undefined
    message.agentRun!.status = 'completed'
    message.agentRun!.finishReason = 'length'
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect.element(screen.getByText('输出达到上限，回复未完成')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__notice')).toBeNull()
    expectCompactInterruption(screen.container)
  })

  it('keeps the interruption visible when the execution timeline is collapsed', async () => {
    const message = interruptedMessage()
    message.status = 'error'
    message.uiState = { timelineCollapsed: true }
    message.agentRun!.interruption = { reason: 'empty_response' }
    message.agentRun!.timeline = [
      { id: 'narration', type: 'message', content: '先前执行播报' },
      {
        id: 'compaction',
        type: 'context_compaction',
        operationId: 'op-compaction',
        status: 'applied'
      }
    ]
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect.element(screen.getByText('模型未返回有效回复')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).not.toBeNull()
    expect(screen.container.textContent).not.toContain('先前执行播报')
    expectCompactInterruption(screen.container)
  })
  it('distinguishes unconfirmed admission from a rejected send', async () => {
    const message = interruptedMessage()
    message.agentRun!.runId = null
    message.agentRun!.interruption = { reason: 'admission_unconfirmed' }
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)
    await expect
      .element(screen.getByText('发送结果尚未确认。已尝试核对，未自动重发。'))
      .toBeVisible()
  })

  it('shows one compact translated reason without a raw error body', async () => {
    const screen = await render(
      <ChatMessageItem message={interruptedMessage()} showTokenUsageDetails={false} />
    )

    await expect.element(screen.getByText('模型服务连接失败')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).not.toBeNull()
    expect(screen.container.querySelector('.agent-run__error')).toBeNull()
    expectCompactInterruption(screen.container)
  })

  it('preserves normal content, images and edits when a stored error replaces the live message', async () => {
    const live = interruptedMessage()
    live.content = '已完成的分析正文'
    live.uiState = { timelineCollapsed: true }
    const hash = 'a'.repeat(64)
    live.agentRun!.toolCalls = [
      {
        id: 'render-pages',
        tool: 'run_command',
        args: {},
        approvalStatus: 'not_required',
        reason: null
      }
    ]
    live.agentRun!.toolResults = [
      {
        callId: 'render-pages',
        tool: 'run_command',
        ok: true,
        result: {
          outputs: [
            {
              name: 'l003-10.png',
              kind: 'image',
              mimeType: 'image/png',
              sizeBytes: 64,
              sha256: hash,
              readPath: `image-artifact://sha256/${hash}`,
              width: 1819,
              height: 2391
            }
          ]
        }
      }
    ]
    const summary: GitTurnDiffSummary = {
      assistantMessageId: live.id,
      files: [{ path: 'derivation.md', status: 'added', stats: { additions: 259, deletions: 0 } }],
      stats: { additions: 259, deletions: 0, fileCount: 1, lineCountsComplete: true },
      truncated: false
    }
    const view = (message: ChatMessage) => (
      <ChatMessageItem
        conversationId="conversation-safe-interruption"
        message={message}
        showTokenUsageDetails={false}
        turnDiffSummary={summary}
      />
    )
    const screen = await render(view(live))
    const article = element(screen.container, '.chat-message--assistant')
    const body = element(article, '.chat-message__body')
    const normalColor = getComputedStyle(body).color
    const normalArticleColor = getComputedStyle(article).color
    const stored = reloadMessage({ ...live, status: 'error' })

    for (const message of [stored, reloadMessage(stored)]) {
      expect(message.status).toBe('error')
      expect(message.agentRun?.status).toBe('failed')
      await screen.rerender(view(message))
      await expect.element(screen.getByText('已完成的分析正文')).toBeVisible()
      await expect.element(screen.getByText('模型服务连接失败')).toBeVisible()
      await expect.element(screen.getByRole('img', { name: 'l003-10.png' }).first()).toBeVisible()
      const edits = element(screen.container, '.edit-summary-card')
      expect(edits.textContent).toContain('derivation.md')
      expect(edits.textContent).toContain('+259')
      expect(getComputedStyle(element(screen.container, '.chat-message__body')).color).toBe(
        normalColor
      )
      expect(getComputedStyle(element(screen.container, '.chat-message--assistant')).color).toBe(
        normalArticleColor
      )
      expectCompactInterruption(screen.container)
    }
  })

  it('keeps the error panel for an ordinary failure without an interruption reason', async () => {
    const message = interruptedMessage()
    message.status = 'error'
    message.agentRun!.interruption = undefined
    message.agentRun!.error = '会话状态无法保存'
    message.content = message.agentRun!.error
    const screen = await render(<ChatMessageItem message={message} showTokenUsageDetails={false} />)

    await expect.element(screen.getByText('会话状态无法保存')).toBeVisible()
    expect(screen.container.querySelector('.agent-run__interruption')).toBeNull()
    const body = element(screen.container, '.chat-message__body')
    expect(getComputedStyle(body).backgroundColor).not.toBe('rgba(0, 0, 0, 0)')
    expect(getComputedStyle(body).borderTopWidth).toBe('1px')
    expect(getComputedStyle(body).padding).toBe('14px 18px')
  })
})
