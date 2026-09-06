import type { HumanInteractionHostApi, HostInvocationResult } from '@mycopilot/host-api'
import type { AgentProposedAction, HumanInteractionRequestSnapshot } from '@mycopilot/protocol'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getFrontendTheme } from '../../../config/frontendTheme'
import {
  deferred,
  fakeHost,
  question,
  submitted
} from '../../humanInteraction/__tests__/humanInteractionFixtures'
import { humanInteractionResponseDisplay } from '../../humanInteraction/humanInteractionState'
import { ConversationSurface } from '../ConversationSurface'
import type { ChatComposerDraft, ChatConversation, ChatMessage } from '../chatTypes'
import '../../../styles/global.css'

const boundary = vi.hoisted(() => ({
  api: undefined as HumanInteractionHostApi | undefined,
  send: vi.fn(),
  stop: vi.fn(),
  draft: vi.fn()
}))
vi.mock('../../../host/hostClient', () => ({
  hostClient: {
    get humanInteraction() {
      return boundary.api
    }
  }
}))
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
  return { useFrontendConfig: () => ({ language: 'zh-CN', t }) }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model-1', displayName: 'Model', supportsImage: true, enabled: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

const approval: AgentProposedAction = {
  type: 'command',
  command: {
    id: 'approval-command',
    command: 'pnpm test',
    cwd: null,
    timeoutMs: null,
    approvalStatus: 'required',
    riskLevel: null,
    reason: '验证项目',
    observe: null
  }
}

function batch(id: string, sequence = 1): HumanInteractionRequestSnapshot {
  const request = question(id, sequence)
  return {
    ...request,
    questions: [
      {
        id: `${id}-one`,
        title: `${id}：选择外观`,
        options: [{ id: `${id}-option`, label: `${id} 明亮外观` }]
      },
      { id: `${id}-two`, title: `${id}：补充需求`, options: null }
    ]
  }
}

function conversation(
  requests: HumanInteractionRequestSnapshot[],
  status: 'running' | 'completed' | 'waiting_for_approval' = 'running'
): ChatConversation {
  return {
    id: 'chat',
    projectId: null,
    modelId: 'model-1',
    title: '问答会话',
    createdAt: 1,
    updatedAt: 2,
    messages: [
      { id: 'initial-user', role: 'user', content: '请完成项目。', createdAt: 1, status: 'sent' },
      {
        id: 'assistant-chat',
        role: 'assistant',
        content: '继续处理任务。',
        createdAt: 2,
        status: status === 'completed' ? 'sent' : 'pending',
        agentRun: {
          runId: 'run-chat',
          status,
          startedAt: 2,
          completedAt: status === 'completed' ? 3 : undefined,
          toolDefinitions: [],
          toolCalls: requests.map((request) => ({
            id: request.toolCallId,
            tool: request.mode === 'sync' ? 'request_user_input' : 'request_user_input_async',
            args: { questions: request.questions },
            approvalStatus: 'not_required',
            reason: null
          })),
          toolResults: requests
            .filter((request) => request.mode === 'async')
            .map((request) => ({
              callId: request.toolCallId,
              tool: 'request_user_input_async',
              ok: true,
              result: { accepted: true, requestId: request.requestId }
            })),
          approvals: status === 'waiting_for_approval' ? [approval] : [],
          fileChangeProposals: [],
          timeline: requests.map((request) => ({
            id: `timeline-${request.requestId}`,
            type: 'tool_call',
            callId: request.toolCallId
          }))
        }
      }
    ]
  }
}

function Workspace({
  value,
  observer = false,
  hasCollaborationApproval = false,
  initialAttachments = []
}: {
  value: ChatConversation
  observer?: boolean
  hasCollaborationApproval?: boolean
  initialAttachments?: ChatComposerDraft['attachments']
}) {
  // The real shell persists text through a ref-only callback. Keep the prop stale until an explicit
  // commit, so an unintended messageSyncKey reset is observable in the real Composer.
  const [draft, setDraft] = useState<ChatComposerDraft>(() => ({
    message: '',
    permissionMode: 'default',
    modelId: 'model-1',
    projectId: null,
    attachments: initialAttachments,
    skills: [],
    queuedMessages: [],
    updatedAt: 0
  }))
  return (
    <div style={{ width: 600, height: 780, display: 'flex', minHeight: 0 }}>
      {observer ? (
        <ConversationSurface
          mode="observer"
          rootConversationId="root"
          conversation={value}
          showTokenUsageDetails={false}
        />
      ) : (
        <ConversationSurface
          mode="interactive"
          conversation={value}
          composerDraft={draft}
          editSelectedModelAvailable
          editSelectedModelSupportsImage
          onComposerDraftChange={setDraft}
          onComposerDraftMessageChange={boundary.draft}
          onSubmitMessage={boundary.send}
          onStopGenerating={boundary.stop}
          onApproveAgentAction={async () => false}
          onRejectAgentAction={async () => false}
          onCancelAgentAction={async () => false}
          hasCollaborationApproval={hasCollaborationApproval}
          permissionModeAvailability={{ custom: true, full: true }}
          showTokenUsageDetails={false}
        />
      )}
    </div>
  )
}

let previousRootStyle: string | null
beforeEach(async () => {
  boundary.send.mockReset()
  boundary.stop.mockReset()
  boundary.draft.mockReset()
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens)))
    document.documentElement.style.setProperty(key, value)
  await page.viewport(1000, 900)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('Human interaction in the real conversation surface', () => {
  it('shows newest async batches and reopens earlier drafts after minimization and conversation switches', async () => {
    const old = batch('old'),
      host = fakeHost([old])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([old])} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await expect.element(panel).toHaveAttribute('data-request-id', 'old')
    await panel.getByRole('navigation').getByRole('button', { name: '下一题' }).click()
    await panel.getByRole('textbox').fill('保留这份旧草稿')
    await panel.getByRole('button', { name: '最小化交互' }).click()
    await expect.poll(() => panel.elements().length).toBe(0)
    const newest = batch('newest', 2)
    host.notify(newest)
    await expect.element(panel).toHaveAttribute('data-request-id', 'newest')
    await expect
      .element(screen.getByRole('button', { name: '交互 · 共 2 题' }).first())
      .toBeVisible()
    screen.container.querySelector<HTMLButtonElement>('[data-human-request-id="old"]')!.click()
    await expect.element(panel).toHaveAttribute('data-request-id', 'old')
    await expect.element(panel.getByText('2 / 2')).toBeVisible()
    await expect.element(panel.getByRole('textbox')).toHaveValue('保留这份旧草稿')
    await screen.rerender(<Workspace value={{ ...conversation([]), id: 'other', messages: [] }} />)
    await expect.poll(() => panel.elements().length).toBe(0)
    await screen.rerender(<Workspace value={conversation([old, newest])} />)
    await expect.element(panel.getByRole('textbox')).toHaveValue('保留这份旧草稿')
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(host.api.ignore).not.toHaveBeenCalled()
    expect(boundary.send).not.toHaveBeenCalled()
  })

  it('lets real approvals preempt a blocking question and restores its draft only after an authoritative refresh', async () => {
    const request = { ...batch('blocking'), mode: 'sync' as const },
      host = fakeHost([request])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([request])} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await expect.element(panel).toBeVisible()
    await panel.getByRole('textbox').fill('审批前填写的内容')
    await screen.rerender(<Workspace value={conversation([request], 'waiting_for_approval')} />)
    await expect.poll(() => panel.elements().length).toBe(0)
    await expect.element(screen.getByRole('dialog')).toHaveClass('agent-approval-dialog')
    const refresh = deferred<
      HostInvocationResult<{
        items: HumanInteractionRequestSnapshot[]
        nextCursor: string | null
      }>
    >()
    host.api.listRequests.mockReturnValueOnce(refresh.promise)
    await screen.rerender(<Workspace value={conversation([request])} />)
    await expect.poll(() => host.api.listRequests.mock.calls.length).toBe(2)
    expect(panel.elements()).toHaveLength(0)
    refresh.resolve({ ok: true, value: { items: [request], nextCursor: null } })
    await expect.element(panel.getByRole('textbox')).toHaveValue('审批前填写的内容')
    await screen.rerender(<Workspace value={conversation([request])} hasCollaborationApproval />)
    await expect.poll(() => panel.elements().length).toBe(0)
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(host.api.ignore).not.toHaveBeenCalled()
  })

  it('isolates pending IME and button events from the batch that preempts their original question', async () => {
    const old = batch('ime-old'),
      host = fakeHost([old])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([old])} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await panel.getByRole('textbox').fill('尚未发送的中文草稿')
    const input = panel.getByRole('textbox').element()
    const submit = panel
      .getByRole('button', { name: '下一项', exact: true })
      .last()
      .element() as HTMLButtonElement
    input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
    host.notify(batch('ime-new', 2))
    await expect.element(panel).toHaveAttribute('data-request-id', 'ime-new')
    expect(input.isConnected).toBe(false)
    expect(submit.isConnected).toBe(false)
    input.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
    )
    input.dispatchEvent(
      new CompositionEvent('compositionend', { data: '迟到的输入法确认', bubbles: true })
    )
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Escape', bubbles: true }))
    submit.click()
    await expect.element(panel.getByRole('textbox')).toHaveValue('')
    screen.container.querySelector<HTMLButtonElement>('[data-human-request-id="ime-old"]')!.click()
    await expect.element(panel.getByRole('textbox')).toHaveValue('尚未发送的中文草稿')
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(host.api.ignore).not.toHaveBeenCalled()
    expect(boundary.send).not.toHaveBeenCalled()
    expect(boundary.stop).not.toHaveBeenCalled()
  })

  it('prioritizes blocking batches and ignores only the selected async batch without sending its draft', async () => {
    const old = batch('old'),
      newer = batch('newer', 2),
      blocking = { ...batch('blocking', 3), mode: 'sync' as const }
    const host = fakeHost([old, newer, blocking])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([old, newer, blocking])} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await expect.element(panel).toHaveAttribute('data-request-id', 'blocking')
    expect(panel.getByRole('button', { name: '忽略全部' }).elements()).toHaveLength(0)
    for (const entry of screen.getByRole('button', { name: '交互 · 共 2 题' }).elements())
      expect(entry).toBeDisabled()
    host.notify({ ...blocking, status: 'cancelled', revision: 1, updatedAt: 4 })
    await expect.element(panel).toHaveAttribute('data-request-id', 'newer')
    await panel.getByRole('textbox').fill('不要发送这份草稿')
    await panel.getByRole('button', { name: '忽略全部' }).click()
    await expect.element(panel).toHaveAttribute('data-request-id', 'old')
    expect(host.api.ignore).toHaveBeenCalledTimes(1)
    expect(host.api.ignore.mock.calls[0][0]).toMatchObject({
      requestId: 'newer',
      conversationId: 'chat',
      expectedRevision: 0
    })
    expect(host.api.ignore.mock.calls[0][0]).not.toHaveProperty('answers')
    expect(host.database.get('old')!.status).toBe('open')
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(screen.container.querySelectorAll('.human-interaction-answer')).toHaveLength(0)
    expect(boundary.send).not.toHaveBeenCalled()
  })

  it('waits for Host admission, hides the settled entry, and renders exactly one complete answer bubble', async () => {
    const request = batch('submit'),
      host = fakeHost([request])
    const pending = deferred<HostInvocationResult<HumanInteractionRequestSnapshot>>()
    host.api.submit.mockReturnValueOnce(pending.promise)
    boundary.api = host.api
    const value = conversation([request])
    const screen = await render(<Workspace value={value} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await panel.getByRole('button', { name: 'submit 明亮外观' }).click()
    await panel.getByRole('navigation').getByRole('button', { name: '下一题' }).click()
    await panel.getByRole('button', { name: '跳过', exact: true }).click()
    await panel.getByRole('button', { name: '提交', exact: true }).click()
    await expect.element(panel).toHaveAttribute('aria-busy', 'true')
    const busy = panel.getByRole('button', { name: '提交' })
    await expect.element(busy).toBeDisabled()
    ;(busy.element() as HTMLButtonElement).click()
    expect(host.api.submit).toHaveBeenCalledTimes(1)
    expect(screen.container.querySelectorAll('.human-interaction-answer')).toHaveLength(0)
    const input = host.api.submit.mock.calls[0][0]
    expect(input).toMatchObject({
      requestId: 'submit',
      conversationId: 'chat',
      expectedRevision: 0,
      answers: [
        { kind: 'option', questionId: 'submit-one', optionId: 'submit-option' },
        { kind: 'skipped', questionId: 'submit-two' }
      ]
    })
    const accepted = submitted(request, input)
    host.database.set(request.requestId, accepted)
    pending.resolve({ ok: true, value: accepted })
    await expect.poll(() => panel.elements().length).toBe(0)
    await expect
      .poll(() => screen.container.querySelectorAll('.human-interaction-answer').length)
      .toBe(1)
    expect(screen.getByRole('button', { name: '交互 · 共 2 题' }).elements()).toHaveLength(0)
    expect(screen.container.querySelector('.human-interaction-answer')!.textContent).toContain(
      'submit：选择外观submit 明亮外观submit：补充需求已跳过'
    )
    const display = humanInteractionResponseDisplay(accepted)!
    const actual = structuredClone(value)
    actual.messages[1].agentRun!.timeline.push({
      id: 'guidance',
      type: 'user_guidance',
      guidanceId: 'guidance-submit',
      clientMessageId: 'human-answer-guidance-submit',
      content: JSON.stringify(display),
      attachments: [],
      status: 'applied',
      createdAt: 4
    })
    await screen.rerender(<Workspace value={actual} />)
    await expect
      .poll(() => screen.container.querySelectorAll('.human-interaction-answer').length)
      .toBe(1)
    expect(actual.messages[1].agentRun!.toolResults).toHaveLength(1)
    expect(boundary.send).not.toHaveBeenCalled()
    expect(boundary.stop).not.toHaveBeenCalled()
  })

  it.each(['before', 'after'] as const)(
    'preserves the Composer draft and attachments when the answer receipt arrives %s its Host User pair',
    async (receiptOrder) => {
      const request = batch('idle'),
        host = fakeHost([request])
      boundary.api = host.api
      const initial = conversation([request], 'completed')
      const screen = await render(
        <Workspace
          value={initial}
          initialAttachments={[
            {
              id: 'draft-file',
              kind: 'file',
              name: 'requirements.txt',
              sizeBytes: 10,
              mimeType: 'text/plain',
              encoding: 'utf8',
              data: 'keep draft'
            }
          ]}
        />
      )
      const panel = screen.getByRole('dialog', { name: '交互', exact: true })
      await expect.element(panel.getByText('本轮已结束，提交回答后继续')).toBeVisible()
      const composer = page.elementLocator(
        screen.container.querySelector<HTMLTextAreaElement>('.chat-composer textarea')!
      )
      await expect.element(composer).not.toBeVisible()
      await panel.getByRole('button', { name: '最小化交互' }).click()
      await expect.element(composer).toBeVisible()
      await composer.fill('正在起草的下一条普通消息')
      expect(boundary.draft).toHaveBeenLastCalledWith(
        expect.objectContaining({ message: '正在起草的下一条普通消息' })
      )
      screen.container.querySelector<HTMLButtonElement>('[data-human-request-id="idle"]')!.click()
      await expect.element(panel).toBeVisible()
      await expect.element(composer).not.toBeVisible()
      const accepted = submitted(request)
      accepted.delivery = {
        ...accepted.delivery!,
        targetRunId: 'next-run',
        userMessageId: 'answer-user'
      }
      const answer: ChatMessage = {
        id: 'answer-user',
        role: 'user',
        humanInteractionDisplay: humanInteractionResponseDisplay(accepted)!,
        content: JSON.stringify(humanInteractionResponseDisplay(accepted)),
        createdAt: 4,
        status: 'sent'
      }
      const nextAssistant: ChatMessage = {
        id: 'next-assistant',
        role: 'assistant',
        content: '',
        createdAt: 5,
        status: 'pending',
        agentRun: {
          runId: 'next-run',
          status: 'running',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          fileChangeProposals: [],
          timeline: []
        }
      }
      if (receiptOrder === 'before') host.notify(accepted)
      await screen.rerender(
        <Workspace value={{ ...initial, messages: [...initial.messages, answer, nextAssistant] }} />
      )
      await expect.element(composer).toHaveValue('正在起草的下一条普通消息')
      expect(screen.container.querySelector('.composer-attachment__name')?.textContent).toBe(
        'requirements.txt'
      )
      if (receiptOrder === 'after') host.notify(accepted)
      await expect
        .poll(() => screen.container.querySelectorAll('.human-interaction-answer').length)
        .toBe(1)
      expect(panel.elements()).toHaveLength(0)
      await expect.element(composer).toBeVisible()
      expect(boundary.send).not.toHaveBeenCalled()

      // A normal user can type the same JSON. Without the Host delivery binding it is an ordinary
      // committed message and must still acknowledge/clear the Composer fast-path draft.
      await screen.rerender(
        <Workspace
          value={{
            ...initial,
            messages: [
              ...initial.messages,
              answer,
              nextAssistant,
              { ...answer, humanInteractionDisplay: undefined, id: 'ordinary-json', createdAt: 6 }
            ]
          }}
        />
      )
      await expect.element(composer).toHaveValue('')
      expect(screen.container.querySelectorAll('.human-interaction-answer')).toHaveLength(1)
    }
  )

  it('replaces the Composer, keeps its local draft across question and approval preemption, and fits a short window', async () => {
    const request = batch('layout'),
      host = fakeHost([])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([])} />)
    const composer = page.elementLocator(
      screen.container.querySelector<HTMLTextAreaElement>('.chat-composer textarea')!
    )
    await composer.fill('尚未发送的输入框草稿')
    host.notify(request)
    await screen.rerender(<Workspace value={conversation([request])} />)
    const panel = screen.getByRole('dialog', { name: '交互', exact: true })
    await expect.element(panel).toBeVisible()
    await expect.element(composer).not.toBeVisible()
    expect(screen.container.querySelector('.conversation-composer-slot')).toHaveAttribute('inert')
    await panel.getByRole('textbox').fill('尚未提交的答案')
    const surface = screen.container.querySelector<HTMLElement>('.conversation-surface')!
    surface.style.height = '350px'
    const footer = screen.container.querySelector<HTMLElement>('.chat-conversation-page__composer')!
    expect(footer.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      surface.getBoundingClientRect().bottom + 1
    )
    expect(footer.clientHeight).toBeGreaterThan(100)
    await screen.rerender(<Workspace value={conversation([request], 'waiting_for_approval')} />)
    await expect.poll(() => panel.elements().length).toBe(0)
    await expect.element(composer).not.toBeVisible()
    await screen.rerender(<Workspace value={conversation([request])} />)
    await expect.element(panel.getByRole('textbox')).toHaveValue('尚未提交的答案')
    await panel.getByRole('button', { name: '最小化交互' }).click()
    await expect.element(composer).toBeVisible()
    await expect.element(composer).toHaveValue('尚未发送的输入框草稿')
    expect(boundary.send).not.toHaveBeenCalled()
    expect(boundary.stop).not.toHaveBeenCalled()
  })

  it('uses strong chat text while retaining gray questions in the submitted answer bubble', async () => {
    const accepted = submitted(batch('contrast')),
      host = fakeHost([accepted])
    boundary.api = host.api
    const screen = await render(<Workspace value={conversation([accepted], 'completed')} />)
    await expect
      .poll(() => screen.container.querySelectorAll('.human-interaction-answer__value').length)
      .toBeGreaterThan(0)
    const value = screen.container.querySelector<HTMLElement>('.human-interaction-answer__value')!
    expect(getComputedStyle(value).color).toBe('rgb(26, 28, 31)')
    const questionText = screen.container.querySelector<HTMLElement>(
      '.human-interaction-answer__question'
    )!
    expect(getComputedStyle(questionText).color).toBe('rgb(79, 86, 96)')
    const userText = screen.container.querySelector<HTMLElement>(
      '.chat-message--user .chat-markdown'
    )!
    const assistantText = screen.container.querySelector<HTMLElement>('.chat-agent-text')!
    expect(getComputedStyle(userText).color).toBe('rgb(26, 28, 31)')
    expect(getComputedStyle(assistantText).color).toBe('rgb(26, 28, 31)')
  })

  it('does not query or expose question controls in a child observer conversation', async () => {
    const request = { ...batch('child'), conversationId: 'child' },
      host = fakeHost([request])
    boundary.api = host.api
    const screen = await render(
      <Workspace observer value={{ ...conversation([request]), id: 'child' }} />
    )
    host.notify(request)
    expect(
      screen.container.querySelectorAll(
        '.human-interaction-panel, .human-interaction-entry, textarea'
      )
    ).toHaveLength(0)
    expect(host.api.listRequests).not.toHaveBeenCalled()
    expect(host.api.submit).not.toHaveBeenCalled()
    expect(host.api.ignore).not.toHaveBeenCalled()
  })
})
