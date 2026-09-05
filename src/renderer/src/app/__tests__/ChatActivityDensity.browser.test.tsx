import { useState, type CSSProperties } from 'react'
import type { AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { ToastProvider } from '../../components/toast/ToastProvider'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import type { ChatConversation, ChatMessage } from '../../features/chat/chatTypes'
import { ChatMessageItem } from '../../features/chat/components/ChatMessageItem'
import { ConversationSurface } from '../../features/chat/ConversationSurface'
import { createComposerDraft } from '../chatMessageFactory'
import '../../styles/global.css'

vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/languageRegistry')
  const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
  return { useFrontendConfig: () => ({ language: 'zh-CN', t }) }
})
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model-1',
        displayName: 'Test model',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      }
    ]
  })
}))
vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
}))
vi.mock('../../features/skills/useSkillCatalog', () => ({
  useSkillCatalog: () => ({ state: { status: 'idle' }, refresh: vi.fn() })
}))
vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
vi.mock('../../features/chat/chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (content: string) => content
}))
vi.mock('../../features/storage/storageClient', () => ({
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn(),
  revealStoredProjectFile: vi.fn()
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const LONG_REASON =
  '检查聊天区的工具调用状态与底部终端同时显示时，较长的中文运行原因能够自然换行，并确保后续行仍然与首行文字对齐而不会挤入图标区域。'

function commandCall(index: number, reason = `验证第${index}个命令`): AgentToolCall {
  return {
    id: `command-${index}`,
    tool: 'run_command',
    args: { command: `echo command-${index}`, reason },
    approvalStatus: 'approved',
    reason: null
  }
}

function commandResult(call: AgentToolCall): AgentToolResult {
  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: { status: 'exited', exitCode: 0, stdout: 'passed\n', stderr: '' }
  }
}

function activityMessage({
  commandCount = 2,
  longReason = false,
  narration = '',
  running = false
}: {
  commandCount?: number
  longReason?: boolean
  narration?: string
  running?: boolean
} = {}): ChatMessage {
  const calls = Array.from({ length: commandCount }, (_, index) =>
    commandCall(index + 1, longReason && index === 0 ? LONG_REASON : undefined)
  )
  const attachmentListCall: AgentToolCall = {
    id: 'attachment-list',
    tool: 'attachments_list',
    args: {},
    approvalStatus: 'not_required',
    reason: null
  }
  return {
    id: 'assistant-activity',
    role: 'assistant',
    content: running ? '' : '检查完成。',
    createdAt: 1,
    status: running ? 'pending' : 'sent',
    uiState: { timelineCollapsed: false },
    agentRun: {
      runId: 'run-activity',
      status: running ? 'running' : 'completed',
      startedAt: 1,
      firstResponseAt: 2,
      completedAt: running ? undefined : 25_001,
      toolDefinitions: [],
      toolCalls: [attachmentListCall, ...calls],
      toolResults: [
        {
          callId: attachmentListCall.id,
          tool: attachmentListCall.tool,
          ok: true,
          result: { attachments: [], total: 0 }
        },
        ...calls.map(commandResult)
      ],
      approvals: [],
      fileChangeProposals: [],
      timeline: [
        ...(narration ? [{ id: 'narration', type: 'message' as const, content: narration }] : []),
        ...[attachmentListCall, ...calls].map((call) => ({
          id: `timeline-${call.id}`,
          type: 'tool_call' as const,
          callId: call.id
        }))
      ]
    }
  }
}

function conversation(message: ChatMessage): ChatConversation {
  return {
    id: 'activity-conversation',
    projectId: null,
    modelId: 'model-1',
    title: '工具调用密度',
    createdAt: 1,
    updatedAt: 25_001,
    messages: [message]
  }
}

function Workspace({ message, bottomOpen = true }: { message: ChatMessage; bottomOpen?: boolean }) {
  const [draft, setDraft] = useState(() => createComposerDraft({ modelId: 'model-1' }))
  return (
    <ToastProvider>
      <div
        className="app-shell"
        data-bottom-open={String(bottomOpen)}
        data-left-open="false"
        data-right-open="false"
        style={
          {
            width: 960,
            height: 900,
            '--left-panel-width': '0px',
            '--right-panel-width': '0px',
            '--bottom-panel-height': bottomOpen ? '450px' : '0px'
          } as CSSProperties
        }
      >
        <main className="main-panel">
          <div className="main-panel__toolbar" data-testid="toolbar" />
          <div className="main-panel__surface">
            <ConversationSurface
              conversation={conversation(message)}
              composerDraft={draft}
              editSelectedModelAvailable
              editSelectedModelSupportsImage
              initialScrollTop={0}
              mode="interactive"
              onComposerDraftChange={setDraft}
              onSubmitMessage={vi.fn()}
              permissionModeAvailability={{ custom: true, full: true }}
              showTokenUsageDetails={false}
            />
          </div>
        </main>
        <aside className="side-panel side-panel--bottom" data-testid="bottom-panel">
          终端区域
        </aside>
      </div>
    </ToastProvider>
  )
}

function requiredElement<T extends HTMLElement>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector)
  if (!element) throw new Error(`Missing fixture element: ${selector}`)
  return element
}

let previousRootStyle: string | null
beforeEach(async () => {
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  await page.viewport(1280, 1000)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('Chat activity density in the real timeline', () => {
  it('keeps consecutive tools compact and aligns expanded commands with the parent label', async () => {
    const screen = await render(
      <ToastProvider>
        <div style={{ width: 720 }}>
          <ChatMessageItem message={activityMessage()} showTokenUsageDetails={false} />
        </div>
      </ToastProvider>
    )
    const run = requiredElement(screen.container, '.agent-run')
    const elapsed = requiredElement(run, '.agent-run__elapsed')
    const attachmentList = requiredElement(run, '.agent-activity--attachment-list')
    const commandGroup = requiredElement<HTMLDetailsElement>(run, '.agent-activity--run-command')
    const summary = requiredElement(commandGroup, ':scope > summary')
    const parentLabel = requiredElement(summary, '.agent-activity__label')

    expect(getComputedStyle(elapsed).rowGap).toBe('6px')
    expect(getComputedStyle(parentLabel).fontWeight).toBe('400')
    expect(summary.getBoundingClientRect().height).toBe(28)
    expect(
      commandGroup.getBoundingClientRect().top - attachmentList.getBoundingClientRect().bottom
    ).toBe(4)
    expect(
      attachmentList.getBoundingClientRect().top - elapsed.getBoundingClientRect().bottom
    ).toBeCloseTo(14, 1)

    await userEvent.click(summary)
    const children = commandGroup.querySelectorAll<HTMLElement>(
      '.run-command-activity__details--group > .agent-activity > summary'
    )
    expect(children).toHaveLength(2)
    expect(children[0].getBoundingClientRect().height).toBe(26)
    expect(
      children[1].getBoundingClientRect().top - children[0].getBoundingClientRect().bottom
    ).toBe(2)
    expect(children[0].getBoundingClientRect().left).toBeCloseTo(
      parentLabel.getBoundingClientRect().left,
      1
    )
    expect(commandGroup.getBoundingClientRect().height).toBe(86)
    await userEvent.click(summary)
    expect(commandGroup.open).toBe(false)
    expect(commandGroup.getBoundingClientRect().height).toBe(28)
  })

  it('wraps long Chinese reasons inside narrow columns with a consistent hanging indent', async () => {
    const screen = await render(
      <ToastProvider>
        <div style={{ width: 320 }}>
          <ChatMessageItem
            message={activityMessage({ longReason: true })}
            showTokenUsageDetails={false}
          />
        </div>
      </ToastProvider>
    )
    const group = requiredElement<HTMLDetailsElement>(
      screen.container,
      '.agent-activity--run-command'
    )
    await userEvent.click(requiredElement(group, ':scope > summary'))
    const summary = requiredElement(
      group,
      '.run-command-activity__details--group > .agent-activity > summary'
    )
    const label = requiredElement(summary, '.agent-activity__label')
    const lines = Array.from(label.getClientRects()).filter((rect) => rect.width > 0)
    expect(lines.length).toBeGreaterThan(2)
    expect(label.textContent).toContain(LONG_REASON)
    for (const line of lines) {
      expect(line.left).toBeCloseTo(lines[0].left, 1)
      expect(line.right).toBeLessThanOrEqual(summary.getBoundingClientRect().right + 1)
    }
    expect(summary.getBoundingClientRect().height).toBeGreaterThan(40)
    expect(group.scrollWidth).toBeLessThanOrEqual(group.clientWidth + 1)
  })

  it('reveals expanded command output above the composer while a half-height bottom panel stays fixed', async () => {
    const narration = Array.from({ length: 12 }, (_, index) => `第${index + 1}步检查已完成。`).join(
      '\n\n'
    )
    const message = activityMessage({ commandCount: 1, narration })
    const screen = await render(<Workspace message={message} />)
    const scroller = requiredElement(screen.container, '.chat-conversation-page__messages')
    const disclosure = requiredElement<HTMLDetailsElement>(scroller, '.agent-activity--run-command')
    const summary = requiredElement(disclosure, ':scope > summary')
    const composer = requiredElement(screen.container, '.chat-conversation-page__composer')
    const bottom = requiredElement(screen.container, '.side-panel--bottom')
    const toolbar = requiredElement(screen.container, '.main-panel__toolbar')
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
    )
    scroller.scrollTop +=
      summary.getBoundingClientRect().bottom - (scroller.getBoundingClientRect().bottom - 16)
    const scrollBefore = scroller.scrollTop
    const composerTop = composer.getBoundingClientRect().top
    const bottomTop = bottom.getBoundingClientRect().top
    const toolbarTop = toolbar.getBoundingClientRect().top
    const collapsedScrollHeight = scroller.scrollHeight
    expect(bottomTop).toBe(450)
    await userEvent.click(summary)
    await vi.waitFor(() => {
      expect(scroller.scrollTop).toBeGreaterThan(scrollBefore)
      expect(disclosure.getBoundingClientRect().bottom).toBeLessThanOrEqual(
        scroller.getBoundingClientRect().bottom - 15
      )
    })
    expect(requiredElement(disclosure, '.run-command-shell__output').textContent).toBe('passed')
    expect(composer.getBoundingClientRect().top).toBe(composerTop)
    expect(bottom.getBoundingClientRect().top).toBe(bottomTop)
    expect(toolbar.getBoundingClientRect().top).toBe(toolbarTop)
    expect(scroller.getBoundingClientRect().bottom).toBeLessThanOrEqual(composerTop)
    await userEvent.click(summary)
    expect(disclosure.open).toBe(false)
    expect(scroller.scrollHeight).toBe(collapsedScrollHeight)
    await screen.rerender(<Workspace message={message} bottomOpen={false} />)
    expect(requiredElement(screen.container, '.agent-activity--run-command')).toBe(disclosure)
    expect(disclosure.open).toBe(false)
  })

  it('preserves expanded group and command identities when streamed tools extend the same group', async () => {
    const screen = await render(<Workspace message={activityMessage({ running: true })} />)
    const group = requiredElement<HTMLDetailsElement>(
      screen.container,
      '.agent-activity--run-command'
    )
    await userEvent.click(requiredElement(group, ':scope > summary'))
    const command = requiredElement<HTMLDetailsElement>(
      group,
      '.run-command-activity__details--group > .agent-activity'
    )
    await userEvent.click(requiredElement(command, ':scope > summary'))
    const output = requiredElement(command, '.run-command-shell__output')
    await screen.rerender(
      <Workspace message={activityMessage({ commandCount: 3, running: true })} />
    )
    expect(requiredElement(screen.container, '.agent-activity--run-command')).toBe(group)
    expect(requiredElement(group, '.run-command-activity__details--group > .agent-activity')).toBe(
      command
    )
    expect(requiredElement(command, '.run-command-shell__output')).toBe(output)
    expect(group.open).toBe(true)
    expect(command.open).toBe(true)
    expect(
      group.querySelectorAll('.run-command-activity__details--group > .agent-activity')
    ).toHaveLength(3)
    expect(group.textContent).toContain('验证第3个命令')
  })
})
