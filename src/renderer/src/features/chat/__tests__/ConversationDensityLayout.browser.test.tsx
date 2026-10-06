import { useState, type CSSProperties } from 'react'
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import type { TranslationKey } from '../../../config/languageRegistry'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import type { ChatConversation, ChatMessage } from '../chatTypes'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import '../../../styles/global.css'
import '../../agentCollaboration/AgentCenterPanel.css'

const { copyText } = vi.hoisted(() => ({
  copyText: vi.fn(async () => undefined)
}))
vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  const translate = (key: TranslationKey) => getTranslation('zh-CN', key)
  return { useFrontendConfig: () => ({ t: translate, language: 'zh-CN' }) }
})
vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [{ id: 'model-1', displayName: 'Model One', enabled: true, supportsImage: true }]
  })
}))
vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], openCreateProjectDialog: vi.fn(async () => null) })
}))
vi.mock('../../skills/useSkillCatalog', () => ({
  useSkillCatalog: () => ({ state: { status: 'idle' }, refresh: vi.fn() })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../components/clipboard', () => ({ copyTextToClipboard: copyText }))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

const [{ ConversationSurface }, { createComposerDraft }] = await Promise.all([
  import('../ConversationSurface'),
  import('../../../app/chatMessageFactory')
])

function conversation(turns = 4, longText = false): ChatConversation {
  return {
    id: 'density-chat',
    title: 'Density conversation',
    projectId: null,
    modelId: 'model-1',
    createdAt: 1,
    updatedAt: 2,
    messagesLoaded: true,
    archivedAt: null,
    unreadAt: null,
    messages: Array.from({ length: turns }, (_, index) => [
      {
        id: `user-${index}`,
        role: 'user' as const,
        content: `用户请求 ${index}`,
        createdAt: 1000 + index,
        status: 'sent' as const
      },
      {
        id: `assistant-${index}`,
        role: 'assistant' as const,
        content: longText
          ? `${'这是一段用于验证窄聊天区换行和滚动的长中文正文，包含 English 和文件路径。'.repeat(8)}\n\n- 第一项\n  - 嵌套说明\n- 第二项`
          : `助手回答 ${index}`,
        createdAt: 2000 + index,
        status: 'sent' as const
      }
    ]).flat()
  }
}

function Workspace({
  middleWidth = 720,
  rightWidth = 200,
  shellWidth,
  shellHeight = 760,
  halfHeight = false,
  turns = 4,
  longText = false,
  observer = false,
  initialConversation,
  collaborationActivities
}: {
  middleWidth?: number
  rightWidth?: number
  shellWidth?: number
  shellHeight?: number
  halfHeight?: boolean
  turns?: number
  longText?: boolean
  observer?: boolean
  initialConversation?: ChatConversation
  collaborationActivities?: CollaborationTimelineActivity[]
}) {
  const [bottomOpen, setBottomOpen] = useState(halfHeight)
  const [draft, setDraft] = useState(() => createComposerDraft({ modelId: 'model-1' }))
  const [chat] = useState(() => initialConversation ?? conversation(turns, longText))
  const common = {
    conversation: chat,
    initialScrollTop: 0,
    showTokenUsageDetails: false,
    collaborationTimelineActivities: collaborationActivities,
    onOpenCollaborationAgent: vi.fn()
  }
  return (
    <div
      className="app-shell"
      data-left-open="true"
      data-right-open="true"
      data-bottom-open={String(bottomOpen)}
      style={
        {
          width: shellWidth ?? middleWidth + 360,
          height: shellHeight,
          '--left-panel-width': '160px',
          '--right-panel-width': `${rightWidth}px`,
          '--bottom-panel-height': bottomOpen ? `${shellHeight / 2}px` : '0px'
        } as CSSProperties
      }
    >
      <aside className="side-panel side-panel--left">
        <button type="button" onClick={() => setBottomOpen((value) => !value)}>
          toggle bottom
        </button>
      </aside>
      <main className="main-panel">
        <div className="main-panel__toolbar">Conversation</div>
        <div className={`main-panel__surface${observer ? ' agent-center__observer' : ''}`}>
          {observer ? (
            <ConversationSurface {...common} mode="observer" rootConversationId="root-chat" />
          ) : (
            <ConversationSurface
              {...common}
              mode="interactive"
              composerDraft={draft}
              onComposerDraftChange={setDraft}
              editSelectedModelAvailable
              editSelectedModelSupportsImage
              onSubmitMessage={vi.fn()}
              onMessageUiStateChange={vi.fn()}
              onContinueInNewTask={vi.fn()}
              permissionModeAvailability={{ custom: true, full: true }}
            />
          )}
        </div>
      </main>
      <aside className="side-panel side-panel--right">Review</aside>
      <aside className="side-panel side-panel--bottom">Terminal</aside>
    </div>
  )
}

function element(selector: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(selector)
  if (!found) throw new Error(`Missing ${selector}`)
  return found
}
const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))

function stoppedMessage(id: string, durationSeconds: number): ChatMessage {
  return {
    id,
    role: 'assistant',
    content: '',
    createdAt: 1000,
    status: 'sent',
    agentRun: {
      runId: `run-${id}`,
      status: 'cancelled',
      userInterrupted: true,
      startedAt: 1000,
      completedAt: 1000 + durationSeconds * 1000,
      toolDefinitions: [],
      toolCalls: [],
      toolResults: [],
      approvals: [],
      fileChangeProposals: [],
      timeline: []
    }
  }
}

function visualPolishConversation() {
  const chat = conversation(1)
  const path = '03_Cases/C001_FRP_Bimolecular_Termination_CSTR/derivation.md'
  const definitions: [string, string, unknown, unknown][] = [
    ['read', 'read_file', { path }, { path }],
    ['search', 'search_code', { query: 'terminationRate', path: 'src' }, { matches: [] }],
    ['map', 'workspace_map', { focusPath: 'src' }, { workspace: { focusPath: 'src' } }],
    ['attachments', 'attachments_list_project', {}, { total: 3, attachments: [] }],
    [
      'command',
      'run_command',
      { command: 'python3 validation/check.py', reason: '复核报告中的公式与单位格式' },
      { status: 'exited', exitCode: 0, stdout: 'All checks passed.' }
    ],
    [
      'edit',
      'apply_patch',
      {
        request: {
          action: 'begin',
          transactionId: 'visual-edit',
          filePath: path,
          operation: 'update'
        }
      },
      { transactionId: 'visual-edit' }
    ]
  ]
  const calls: AgentToolCall[] = definitions.map(([id, tool, args]) => ({
    id,
    tool,
    args,
    approvalStatus: 'not_required',
    reason: null
  }))
  const results: AgentToolResult[] = definitions.map(([id, tool, , result]) => ({
    callId: id,
    tool,
    ok: true,
    result
  }))
  chat.messages[0].content = '请核对这份报告的公式和验证记录，并整理需要修改的地方。'
  chat.messages[1] = {
    id: 'visual-assistant',
    role: 'assistant',
    content: '检查已完成，报告中的公式与单位格式已统一，验证结果通过。',
    createdAt: 2000,
    status: 'sent',
    uiState: { timelineCollapsed: false },
    agentRun: {
      runId: 'visual-run',
      status: 'completed',
      startedAt: 1000,
      completedAt: 93000,
      toolDefinitions: [],
      toolCalls: calls,
      toolResults: results,
      approvals: [],
      fileChangeProposals: [],
      fileChanges: [
        {
          schemaVersion: 1,
          transactionId: 'visual-edit',
          conversationId: chat.id,
          projectId: null,
          filePath: path,
          operation: 'update',
          updateStrategy: 'modify',
          status: 'applied',
          baseRevision: null,
          additions: 33,
          deletions: 11,
          lineCount: 120,
          byteCount: 2048,
          mutationCount: 1,
          nextMutationIndex: 1,
          statsFinal: true,
          summary: null,
          createdAt: 1000,
          updatedAt: 90000
        }
      ],
      timeline: [
        {
          id: 'visual-narration',
          type: 'message',
          content: '我会先读取报告和相关验证记录，再逐项检查公式，保留原来的推导结构。',
          traceSequence: 1
        },
        ...calls.map((call, index) => ({
          id: `visual-${call.id}`,
          type: 'tool_call' as const,
          callId: call.id,
          identity: { type: 'builtin' as const, toolName: call.tool },
          traceSequence: index * 2 + 2
        }))
      ]
    }
  }
  const activities: CollaborationTimelineActivity[] = [
    {
      activityId: 'visual-review-completed',
      agentId: 'visual-reviewer',
      ownerAgentId: `root:${chat.id}`,
      ownerConversationId: chat.id,
      anchorMessageId: 'visual-assistant',
      traceBoundarySequence: 20,
      taskNameSnapshot: '公式复核',
      semantic: 'completed',
      sequence: 20,
      occurredAt: 91000,
      runId: 'visual-run',
      turnId: null,
      taskMessageId: 'visual-task'
    }
  ]
  chat.messages[1].agentRun!.collaborationTimelineActivities = activities
  return { chat, activities }
}

function messageGap(before: string, after: string) {
  return (
    element(`[data-message-id="${after}"]`).getBoundingClientRect().top -
    element(`[data-message-id="${before}"]`).getBoundingClientRect().bottom
  )
}

async function scrollToEnd() {
  // Let ConversationSurface restore the initial scroll position before scrolling explicitly.
  await frame()
  const scroller = element('.chat-conversation-page__messages')
  scroller.scrollTop = scroller.scrollHeight
  await frame()
  return scroller
}

let previousRootStyle: string | null
beforeEach(async () => {
  copyText.mockClear()
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [name, value] of Object.entries(getFrontendCssVariables()))
    document.documentElement.style.setProperty(name, value)
  await page.viewport(1600, 1000)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('Conversation density and adaptive message layout', () => {
  it.each([480, 720])(
    'keeps consecutive stopped and resumed status rows compact at %s px',
    async (middleWidth) => {
      const chat = conversation(1)
      const resumed: ChatMessage = {
        ...stoppedMessage('resumed', 12),
        status: 'pending',
        agentRun: {
          ...stoppedMessage('resumed', 12).agentRun!,
          status: 'running',
          userInterrupted: undefined,
          completedAt: undefined,
          firstResponseAt: 1001
        }
      }
      chat.messages = [
        chat.messages[0],
        stoppedMessage('stopped-0', 0),
        stoppedMessage('stopped-1', 2),
        stoppedMessage('stopped-2', 2),
        resumed
      ]
      await render(<Workspace middleWidth={middleWidth} initialConversation={chat} />)

      expect(document.querySelectorAll('.agent-run__elapsed')).toHaveLength(4)
      for (const [before, after] of [
        ['stopped-0', 'stopped-1'],
        ['stopped-1', 'stopped-2'],
        ['stopped-2', 'resumed']
      ]) {
        expect(messageGap(before, after)).toBe(12)
        const previousHeader = element(`[data-message-id="${before}"] .agent-run__elapsed`)
        const nextHeader = element(`[data-message-id="${after}"] .agent-run__elapsed`)
        expect(
          nextHeader.getBoundingClientRect().top - previousHeader.getBoundingClientRect().bottom
        ).toBeLessThanOrEqual(20)
        const nextBox = nextHeader.getBoundingClientRect()
        expect(
          document
            .elementFromPoint(nextBox.left + 8, nextBox.top + 8)
            ?.closest('[data-message-id]')
            ?.getAttribute('data-message-id')
        ).toBe(after)
      }
      expect(messageGap('user-0', 'stopped-0')).toBe(40)
    }
  )

  it('restores ordinary separation when a stopped timeline expands or a user message intervenes', async () => {
    const chat = conversation(2)
    const stopped = stoppedMessage('stopped', 16)
    stopped.agentRun!.toolCalls = [
      {
        id: 'read-call',
        tool: 'read_file',
        args: { path: 'notes.md' },
        approvalStatus: 'not_required',
        reason: null
      }
    ]
    stopped.agentRun!.toolResults = [
      { callId: 'read-call', tool: 'read_file', ok: true, result: { path: 'notes.md' } }
    ]
    stopped.agentRun!.timeline = [{ id: 'read-timeline', type: 'tool_call', callId: 'read-call' }]
    chat.messages = [
      chat.messages[0],
      stopped,
      chat.messages[1],
      stoppedMessage('stopped-next', 2),
      chat.messages[2]
    ]
    await render(<Workspace observer initialConversation={chat} />)

    expect(messageGap('stopped', 'assistant-0')).toBe(12)
    expect(messageGap('assistant-0', 'stopped-next')).toBe(40)
    expect(messageGap('stopped-next', 'user-1')).toBe(40)
    const header = page.elementLocator(element('[data-message-id="stopped"] .agent-run__elapsed'))
    await header.click()
    await expect.poll(() => messageGap('stopped', 'assistant-0')).toBe(40)
    await header.click()
    await expect.poll(() => messageGap('stopped', 'assistant-0')).toBe(12)
  })

  it.each([
    { width: 480, padding: 24, rail: false },
    { width: 720, padding: 32, rail: true },
    { width: 900, padding: 40, rail: true }
  ])(
    'uses the same $width px middle-column layout at different application widths',
    async ({ width, padding, rail }) => {
      const screen = await render(<Workspace middleWidth={width} />)
      const scroller = element('.chat-conversation-page__messages')
      for (const viewportWidth of [1600, width + 360]) {
        await page.viewport(viewportWidth, 1000)
        expect(getComputedStyle(scroller).paddingLeft).toBe(`${padding}px`)
        expect(getComputedStyle(scroller).paddingRight).toBe(`${padding}px`)
        const textBounds = element('.chat-message--assistant').getBoundingClientRect()
        const composerBounds = element('.chat-composer').getBoundingClientRect()
        expect(Math.abs(textBounds.left - composerBounds.left)).toBeLessThanOrEqual(1)
        expect(Math.abs(textBounds.right - composerBounds.right)).toBeLessThanOrEqual(1)
        const navigation = element('.conversation-turn-navigation')
        expect(getComputedStyle(navigation).display === 'none').toBe(!rail)
        if (rail) {
          const row = element('.conversation-turn-navigation__row').getBoundingClientRect()
          const text = element('.chat-message--assistant').getBoundingClientRect()
          expect(row.right + 2).toBeLessThanOrEqual(text.left)
        }
      }
      await screen.unmount()
    }
  )

  it.each([
    { mode: 'light', theme: classicLightTheme },
    { mode: 'dark', theme: classicDarkTheme }
  ])(
    'keeps a bounded reading column aligned with the composer as the sidebar resizes in $mode mode',
    async ({ mode, theme }) => {
      for (const [name, value] of Object.entries(getFrontendCssVariables(frontendConfig, theme))) {
        document.documentElement.style.setProperty(name, value)
      }
      await page.viewport(1920, 1100)
      const chat = conversation(1, true)
      const screen = await render(
        <Workspace
          shellWidth={1560}
          rightWidth={200}
          initialConversation={chat}
          shellHeight={1040}
        />
      )
      let wideReadingWidth: number | undefined
      const originalComposer = element('.chat-composer')
      const originalText = element('.chat-message--assistant')
      const cases = [
        { name: 'wide', shellWidth: 1560, rightWidth: 200 },
        { name: 'wider', shellWidth: 1840, rightWidth: 200 },
        { name: 'sidebar-expanded', shellWidth: 1560, rightWidth: 600 },
        { name: 'narrow', shellWidth: 1560, rightWidth: 1080 },
        { name: 'restored', shellWidth: 1560, rightWidth: 200 }
      ]
      for (const fixture of cases) {
        await screen.rerender(
          <Workspace
            shellWidth={fixture.shellWidth}
            rightWidth={fixture.rightWidth}
            initialConversation={chat}
            shellHeight={1040}
          />
        )
        await expect
          .poll(() => Math.round(element('.main-panel').getBoundingClientRect().width))
          .toBe(fixture.shellWidth - fixture.rightWidth - 160)
        const text = element('.chat-message--assistant')
        const composer = element('.chat-composer')
        const scroller = element('.chat-conversation-page__messages')
        expect(composer).toBe(originalComposer)
        expect(text).toBe(originalText)
        const textBounds = text.getBoundingClientRect()
        const composerBounds = composer.getBoundingClientRect()
        expect(Math.abs(textBounds.left - composerBounds.left)).toBeLessThanOrEqual(1)
        expect(Math.abs(textBounds.right - composerBounds.right)).toBeLessThanOrEqual(1)
        const panel = element('.main-panel').getBoundingClientRect()
        expect(
          Math.abs((textBounds.left + textBounds.right) / 2 - (panel.left + panel.right) / 2)
        ).toBeLessThanOrEqual(1)
        expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
        expect(composer.scrollWidth).toBeLessThanOrEqual(composer.clientWidth + 1)
        expect(getComputedStyle(element('.chat-message--assistant .chat-markdown')).fontSize).toBe(
          '15px'
        )
        if (fixture.name === 'wide') wideReadingWidth = textBounds.width
        else if (fixture.name === 'wider' || fixture.name === 'restored')
          expect(textBounds.width).toBeCloseTo(wideReadingWidth!, 1)
        else expect(textBounds.width).toBeLessThan(wideReadingWidth!)
        if (fixture.name !== 'wider' && fixture.name !== 'restored') {
          scroller.scrollTop = 0
          await frame()
          await page.screenshot({
            element: element('.main-panel'),
            path: `../../../../../../.cache/conversation-layout-refinement/width-${mode}-${fixture.name}.png`
          })
        }
      }
    }
  )

  it('keeps the hover path and copy hit area clear of the next message', async () => {
    await render(<Workspace middleWidth={720} />)
    const message = element('[data-message-id="assistant-0"]')
    const next = element('[data-message-id="user-1"]')
    const action = message.querySelector<HTMLButtonElement>('[aria-label="复制消息"]')!
    await userEvent.hover(message)
    const box = message.getBoundingClientRect()
    const actionBox = action.getBoundingClientRect()
    expect(actionBox.width).toBe(24)
    expect(actionBox.height).toBe(24)
    expect(next.getBoundingClientRect().top - box.bottom).toBe(40)
    expect(actionBox.bottom + 3).toBeLessThan(next.getBoundingClientRect().top)
    // Move through the actual pseudo-element bridge, then click without a layout-stability wait.
    await page.elementLocator(message).hover({ position: { x: 8, y: box.height + 3 }, force: true })
    expect(document.elementFromPoint(box.left + 8, box.bottom + 3)).toBe(message)
    await page.elementLocator(action).click({ force: true })
    expect(copyText).toHaveBeenCalledExactlyOnceWith('助手回答 0')
    const nextBody = next.querySelector<HTMLElement>('.chat-message__body')!.getBoundingClientRect()
    expect(
      document.elementFromPoint(nextBody.left + 8, nextBody.top + 8)?.closest('[data-message-id]')
    ).toBe(next)
  })

  it('keeps the final actions reachable above the fixed composer with a half-height terminal', async () => {
    await render(<Workspace middleWidth={480} halfHeight longText />)
    const composer = element('.chat-conversation-page__composer')
    const composerBox = composer.getBoundingClientRect().toJSON()
    const terminalBox = element('.side-panel--bottom').getBoundingClientRect().toJSON()
    const scroller = await scrollToEnd()
    const action = element('[data-message-id="assistant-3"] [aria-label="复制消息"]')
    action.focus({ preventScroll: true })
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
    expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
    await userEvent.keyboard('{Enter}')
    expect(copyText).toHaveBeenCalledOnce()
    expect(composer.getBoundingClientRect().toJSON()).toEqual(composerBox)
    expect(element('.side-panel--bottom').getBoundingClientRect().toJSON()).toEqual(terminalBox)
    expect(composer.getBoundingClientRect().bottom).toBeLessThanOrEqual(terminalBox.top)
    await page.getByRole('button', { name: 'toggle bottom' }).click()
    await expect.poll(() => composer.getBoundingClientRect().bottom).toBe(760)
    await scrollToEnd()
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
  })

  it('bounds a long turn navigator to the shortened message viewport and keeps its last turn reachable', async () => {
    await render(<Workspace middleWidth={720} halfHeight turns={60} />)
    const region = element('.chat-conversation-page__messages-region')
    const list = element('.conversation-turn-navigation__list')
    await expect.poll(() => list.scrollHeight > list.clientHeight).toBe(true)
    expect(list.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      region.getBoundingClientRect().top
    )
    expect(list.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      region.getBoundingClientRect().bottom
    )
    list.scrollTop = list.scrollHeight
    await frame()
    const last = list.lastElementChild as HTMLButtonElement
    expect(last.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      list.getBoundingClientRect().bottom
    )
    await page.elementLocator(last).click()
    await expect
      .poll(() => element('.chat-conversation-page__messages').scrollTop)
      .toBeGreaterThan(1000)
  })

  it('applies the narrow layout to a read-only observer without adding a composer', async () => {
    await render(<Workspace middleWidth={320} observer longText />)
    const scroller = await scrollToEnd()
    expect(getComputedStyle(scroller).paddingLeft).toBe('14px')
    expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
    expect(getComputedStyle(element('.conversation-turn-navigation')).display).toBe('none')
    expect(document.querySelector('.chat-composer')).toBeNull()
    const action = element('[data-message-id="assistant-3"] [aria-label="复制消息"]')
    expect(action.getBoundingClientRect().bottom + 3).toBeLessThanOrEqual(
      scroller.getBoundingClientRect().bottom
    )
  })

  it.each([
    { width: 780, dark: false },
    { width: 780, dark: true },
    { width: 320, dark: false },
    { width: 320, dark: true }
  ])(
    'keeps a complete conversation visually contained at $width px (dark=$dark)',
    async ({ width, dark }) => {
      for (const [name, value] of Object.entries(
        getFrontendCssVariables(frontendConfig, dark ? classicDarkTheme : classicLightTheme)
      )) {
        document.documentElement.style.setProperty(name, value)
      }
      await page.viewport(1600, 1100)
      const { chat, activities } = visualPolishConversation()
      const screen = await render(
        <Workspace
          middleWidth={width}
          shellHeight={1040}
          initialConversation={chat}
          collaborationActivities={activities}
        />
      )
      const header = element('.basic-tool-activity__header')
      const overflow = header.querySelector<HTMLElement>(
        `.basic-tool-activity__summary-overflow--${width < 480 ? 'compact' : 'wide'}`
      )!
      expect(getComputedStyle(overflow).display).not.toBe('none')
      expect(overflow.getBoundingClientRect().right).toBeLessThanOrEqual(
        header.getBoundingClientRect().right
      )
      expect(getComputedStyle(header).fontSize).toBe('14px')
      await userEvent.click(header)
      expect(header).toHaveAttribute('aria-expanded', 'true')
      expect(header.textContent).toContain('已读取 1 个文件')
      expect(header.textContent).toContain('已查找 1 次')
      expect(header.textContent).toContain('已编辑 1 个文件')
      expect(header.textContent).toContain('已运行 1 个命令')
      const scroller = element('.chat-conversation-page__messages')
      scroller.scrollTop = 0
      await frame()
      expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
      expect(element('.chat-composer').scrollWidth).toBeLessThanOrEqual(
        element('.chat-composer').clientWidth + 1
      )
      const leaves = screen.container.querySelectorAll<HTMLElement>('[data-tool-anchor]')
      expect(leaves).toHaveLength(6)
      expect([...leaves].every((leaf) => !leaf.hidden)).toBe(true)
      expect(screen.getByRole('button', { name: /公式复核/ })).toBeVisible()
      expect(element('.file-change-activity__path').textContent).toBe('derivation.md')
      for (const row of leaves) expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth + 1)
      await page.screenshot({
        element: element('.main-panel'),
        path: `../../../../../../.cache/timeline-visual-polish/conversation-${dark ? 'dark' : 'light'}-${width}.png`
      })
    }
  )
})
