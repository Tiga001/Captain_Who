import type { CSSProperties } from 'react'
import type { AgentFileChangeSnapshot, AgentToolCall, AgentToolResult } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { getTranslation, type TranslationKey } from '../../../config/frontendTranslations'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import type { ChatAgentRunView, ChatMessage } from '../chatTypes'
import type { WorkspaceReferenceTarget } from '../workspaceMentions'
import type { CollaborationTimelineActivity } from '../../agentCollaboration/CollaborationTimelineActivity'
import { AgentRunView } from '../components/AgentRunView'
import '../ChatConversationPage.agent.css'
import '../ChatConversationPage.results.css'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    language: 'zh-CN',
    t: (key: TranslationKey) => getTranslation('zh-CN', key)
  })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../storage/storageClient', () => ({
  revealStoredProjectFile: vi.fn(),
  loadAttachmentImage: vi.fn(),
  loadImageFile: vi.fn()
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

afterEach(() => vi.restoreAllMocks())

function call(id: string, tool = 'run_command', args?: unknown): AgentToolCall {
  return {
    id,
    tool,
    args: args ?? { command: `printf '${id}'`, reason: `验证 ${id}` },
    approvalStatus: 'not_required',
    reason: null
  }
}

function result(call: AgentToolCall): AgentToolResult {
  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result:
      call.tool === 'run_command'
        ? { status: 'exited', exitCode: 0, stdout: Array(35).fill(`output ${call.id}`).join('\n') }
        : { path: (call.args as { path?: string }).path }
  }
}

function run(calls: AgentToolCall[], completed: AgentToolCall[] = calls): ChatAgentRunView {
  return {
    runId: 'basic-run',
    status: 'running',
    startedAt: Date.now(),
    firstResponseAt: 1,
    toolDefinitions: [],
    toolCalls: calls,
    toolResults: completed.map(result),
    approvals: [],
    fileChangeProposals: [],
    timeline: calls.map((item, index) => ({
      id: `marker-${item.id}`,
      type: 'tool_call',
      callId: item.id,
      identity: { type: 'builtin', toolName: item.tool },
      traceSequence: index * 2
    }))
  }
}

function view(
  agentRun: ChatAgentRunView,
  {
    activities = [],
    dark = false,
    mode = 'interactive',
    width = 780,
    collapsed = false,
    onOpenWorkspaceReference
  }: {
    activities?: CollaborationTimelineActivity[]
    dark?: boolean
    mode?: 'interactive' | 'observer'
    width?: number
    collapsed?: boolean
    onOpenWorkspaceReference?: (target: WorkspaceReferenceTarget) => void
  } = {}
) {
  const message: ChatMessage = {
    id: 'basic-message',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status: 'pending',
    agentRun,
    uiState: { timelineCollapsed: collapsed }
  }
  return (
    <div
      className="basic-tool-test-panel"
      style={
        {
          ...getFrontendCssVariables(frontendConfig, dark ? classicDarkTheme : classicLightTheme),
          width,
          boxSizing: 'border-box',
          fontFamily: 'var(--mc-font-family)',
          padding: 20,
          color: 'var(--mc-color-text-primary)',
          background: 'var(--mc-color-surface-main-panel)'
        } as CSSProperties
      }
    >
      <AgentRunView
        collaborationTimelineActivities={activities}
        conversationId="basic-conversation"
        message={message}
        mode={mode}
        observerRootConversationId={mode === 'observer' ? 'root-conversation' : undefined}
        onOpenCollaborationAgent={() => {}}
        onOpenWorkspaceReference={onOpenWorkspaceReference}
      />
    </div>
  )
}

const header = (root: ParentNode) =>
  root.querySelector<HTMLButtonElement>('.basic-tool-activity__header')!
const leaf = (root: ParentNode, id: string) =>
  root.querySelector<HTMLElement>(`[data-tool-anchor="marker-${id}"]`)!

describe('mixed tool timeline', () => {
  it.each(['interactive', 'observer'] as const)(
    'opens the original read file from singleton and aggregated %s timelines',
    async (mode) => {
      const filePath = '/workspace/reports/summary.md'
      const read = call('read-file-preview', 'read_file', { path: filePath })
      const onOpenWorkspaceReference = vi.fn()
      const options = { mode, onOpenWorkspaceReference }
      const screen = await render(view(run([read]), options))
      const filename = screen.getByRole('button', { name: filePath, exact: true })
      const filenameNode = filename.element()
      const readLeaf = leaf(screen.container, read.id)
      const assertReadAlignment = () => {
        const icon = readLeaf.querySelector<HTMLElement>('.agent-activity__icon')!
        const action = readLeaf.querySelector<HTMLElement>('.read-activity__action')!
        const iconRect = icon.getBoundingClientRect()
        const actionRect = action.getBoundingClientRect()
        expect(
          Math.abs(iconRect.top + iconRect.height / 2 - actionRect.top - actionRect.height / 2)
        ).toBeLessThanOrEqual(3)
        expect(iconRect.right).toBeLessThanOrEqual(actionRect.left)
      }
      assertReadAlignment()
      await userEvent.click(filename)
      const expectedTarget = {
        source: 'read-tool',
        filePath,
        conversationId: 'basic-conversation',
        assistantMessageId: 'basic-message',
        callId: read.id
      }
      expect(onOpenWorkspaceReference).toHaveBeenLastCalledWith(expectedTarget)

      const command = call('after-read')
      await screen.rerender(view(run([read, command]), options))
      await userEvent.click(header(screen.container))
      expect(leaf(screen.container, read.id)).toBe(readLeaf)
      expect(filename.element()).toBe(filenameNode)
      assertReadAlignment()
      await userEvent.click(filename)
      expect(onOpenWorkspaceReference).toHaveBeenLastCalledWith(expectedTarget)
      expect(header(screen.container).getAttribute('aria-expanded')).toBe('true')
    }
  )

  it('updates a single collapsed header through parallel activity and completion without moving history', async () => {
    const read = call('read', 'read_file', { path: 'src/deeply/nested/Timeline.tsx' })
    const command = call('test')
    const initial = run([read, command], [])
    const screen = await render(view(initial))
    const element = header(screen.container)
    expect(element.textContent).toContain('正在运行')
    expect(element.textContent).toContain('验证 test')
    expect(leaf(screen.container, 'read').hidden).toBe(true)
    expect(leaf(screen.container, 'test').hidden).toBe(true)

    await screen.rerender(view({ ...initial, toolResults: [result(command)] }))
    expect(header(screen.container)).toBe(element)
    expect(element.textContent).toContain('正在读取')
    await screen.rerender(view({ ...initial, toolResults: [result(command), result(read)] }))
    expect(element.textContent).not.toContain('正在')
    expect(element.querySelector('.agent-running-text')).toBeNull()
    await userEvent.click(element)
    expect(element.getAttribute('aria-expanded')).toBe('true')
    expect(leaf(screen.container, 'read').hidden).toBe(false)
    expect(leaf(screen.container, 'test').hidden).toBe(false)
    const anchors = [...screen.container.querySelectorAll('[data-tool-anchor]')]
    expect(anchors.map((item) => item.getAttribute('data-tool-anchor'))).toEqual([
      'marker-read',
      'marker-test'
    ])
  })

  it('preserves open command output and scroll through singleton promotion, appending and collapse', async () => {
    const first = call('first')
    const second = call('second', 'read_file', { path: 'source.ts' })
    const third = call('third')
    const screen = await render(view(run([first])))
    await userEvent.click(screen.container.querySelector('summary')!)
    const details = leaf(screen.container, 'first').querySelector('details')!
    await vi.waitFor(() => expect(details.open).toBe(true))
    const output = screen.container.querySelector<HTMLElement>('.run-command-shell__output')!
    output.scrollTop = 60
    output.dispatchEvent(new Event('scroll'))
    const scrollTop = output.scrollTop

    await screen.rerender(view(run([first, second])))
    expect(header(screen.container).getAttribute('aria-expanded')).toBe('true')
    expect(leaf(screen.container, 'first').querySelector('details')).toBe(details)
    expect(screen.container.querySelector('.run-command-shell__output')).toBe(output)
    expect(output.scrollTop).toBe(scrollTop)
    await screen.rerender(view(run([first, second, third])))
    expect(leaf(screen.container, 'third').hidden).toBe(false)
    await userEvent.click(header(screen.container))
    expect(leaf(screen.container, 'first').hidden).toBe(true)
    await userEvent.click(header(screen.container))
    expect(details.open).toBe(true)
    expect(output.scrollTop).toBe(scrollTop)
  })

  it('inherits expansion through a late narration split and merge while keeping leaf DOM', async () => {
    const calls = [call('a'), call('b'), call('c'), call('d')]
    const initial = run(calls)
    const screen = await render(view(initial))
    await userEvent.click(header(screen.container))
    const lastLeaf = leaf(screen.container, 'd')
    await userEvent.click(lastLeaf.querySelector('summary')!)
    const output = lastLeaf.querySelector('.run-command-shell__output')!
    const split: ChatAgentRunView = {
      ...initial,
      timeline: [
        ...initial.timeline!.slice(0, 2),
        { id: 'narration', type: 'message', content: '接下来核对结果。' },
        ...initial.timeline!.slice(2)
      ]
    }
    await screen.rerender(view(split))
    expect(screen.container.querySelectorAll('.basic-tool-activity__header')).toHaveLength(2)
    expect(leaf(screen.container, 'd')).toBe(lastLeaf)
    expect(lastLeaf.hidden).toBe(false)
    expect(lastLeaf.querySelector('.run-command-shell__output')).toBe(output)
    await screen.rerender(view(initial))
    expect(screen.container.querySelectorAll('.basic-tool-activity__header')).toHaveLength(1)
    expect(lastLeaf.querySelector('.run-command-shell__output')).toBe(output)
    expect(lastLeaf.querySelector('details')!.open).toBe(true)
  })

  it('preserves open leaves around a late collaboration boundary in observer view', async () => {
    const calls = [call('a'), call('b'), call('c'), call('d')]
    const initial = run(calls)
    const screen = await render(view(initial, { mode: 'observer' }))
    await userEvent.click(header(screen.container))
    const lastLeaf = leaf(screen.container, 'd')
    const activity: CollaborationTimelineActivity = {
      activityId: 'child-done',
      agentId: 'child',
      ownerAgentId: 'root:basic-conversation',
      ownerConversationId: 'basic-conversation',
      anchorMessageId: 'basic-message',
      occurredAt: Date.now(),
      traceBoundarySequence: 3,
      sequence: 1,
      runId: 'child-run',
      semantic: 'completed',
      taskNameSnapshot: '检查员',
      turnId: 'child-turn',
      taskMessageId: 'child-task'
    }
    await screen.rerender(view(initial, { activities: [activity], mode: 'observer' }))
    expect(screen.container.querySelectorAll('.basic-tool-activity__header')).toHaveLength(2)
    expect(lastLeaf).toBe(leaf(screen.container, 'd'))
    expect(lastLeaf.hidden).toBe(false)
    expect(screen.container.textContent).toContain('检查员')
  })

  it('retains visible singleton details when a collapsed span splits and grows again', async () => {
    const first = call('first')
    const second = call('second')
    const third = call('third')
    const initial = run([first, second])
    const screen = await render(view(initial))
    await userEvent.click(header(screen.container))
    await userEvent.click(leaf(screen.container, 'second').querySelector('summary')!)
    await userEvent.click(header(screen.container))
    const split = run([first, second])
    split.timeline!.splice(1, 0, { id: 'split', type: 'message', content: '继续验证。' })
    await screen.rerender(view(split))
    const singleton = leaf(screen.container, 'second')
    expect(singleton.hidden).toBe(false)
    expect(singleton.querySelector('details')!.open).toBe(true)
    const grown = run([first, second, third])
    grown.timeline!.splice(1, 0, { id: 'split', type: 'message', content: '继续验证。' })
    await screen.rerender(view(grown))
    expect(header(screen.container).getAttribute('aria-expanded')).toBe('true')
    expect(leaf(screen.container, 'second')).toBe(singleton)
    expect(leaf(screen.container, 'third').hidden).toBe(false)
  })

  it('keeps leaf details when the completed run is collapsed then reopened', async () => {
    const initial = { ...run([call('a'), call('b')]), status: 'completed' as const }
    const screen = await render(view(initial))
    await userEvent.click(header(screen.container))
    const commandLeaf = leaf(screen.container, 'a')
    await userEvent.click(commandLeaf.querySelector('summary')!)
    const output = commandLeaf.querySelector('.run-command-shell__output')
    await screen.rerender(view(initial, { collapsed: true }))
    expect(commandLeaf.hidden).toBe(true)
    expect(screen.container.querySelector('.basic-tool-activity__header')).toBeNull()
    await screen.rerender(view(initial))
    expect(commandLeaf.hidden).toBe(false)
    expect(commandLeaf.querySelector('.run-command-shell__output')).toBe(output)
    expect(commandLeaf.querySelector('details')!.open).toBe(true)
  })

  it('updates staged editing in place with colored counts and compact chronological rows', async () => {
    await page.viewport(1000, 740)
    const path = '03_Cases/C001_FRP_Bimolecular_Termination_CSTR/derivation.md'
    const read = call('read', 'read_file', { path })
    const begin = call('begin', 'apply_patch', {
      request: { action: 'begin', transactionId: 'edit', filePath: path, operation: 'update' }
    })
    const append = call('append', 'apply_patch', {
      request: { action: 'append', transactionId: 'edit', content: 'new text' }
    })
    const check = call('check')
    const snapshot: AgentFileChangeSnapshot = {
      schemaVersion: 1,
      transactionId: 'edit',
      conversationId: 'basic-conversation',
      projectId: null,
      filePath: path,
      operation: 'update',
      updateStrategy: 'modify',
      status: 'drafting',
      baseRevision: null,
      additions: 11,
      deletions: 2,
      lineCount: 33,
      byteCount: 100,
      mutationCount: 1,
      nextMutationIndex: 1,
      statsFinal: false,
      summary: null,
      createdAt: 1,
      updatedAt: 2
    }
    const initial = { ...run([read, begin], [read]), fileChanges: [snapshot] }
    const screen = await render(view(initial))
    const top = header(screen.container)
    expect(top.textContent).toContain('正在编辑')
    expect(top.textContent).toContain('derivation.md')
    await userEvent.click(top)
    const editLeaf = leaf(screen.container, 'begin')
    const counter = editLeaf.querySelector('.file-change-activity__additions')
    const next = {
      ...run([read, begin, check, append], [read, check]),
      fileChanges: [{ ...snapshot, additions: 33, updatedAt: 3 }]
    }
    await screen.rerender(view(next))
    expect(header(screen.container)).toBe(top)
    expect(leaf(screen.container, 'begin')).toBe(editLeaf)
    expect(editLeaf.querySelector('.file-change-activity__additions')).toBe(counter)
    expect(screen.container.querySelectorAll('[data-tool-anchor]')).toHaveLength(3)
    expect(editLeaf.querySelector('.file-change-activity__path')?.textContent).toBe('derivation.md')
    for (const element of screen.container.querySelectorAll('.file-change-activity__additions')) {
      expect(getComputedStyle(element).color).toBe('rgb(22, 163, 74)')
      expect(element.textContent).toContain('+33')
    }
    await vi.waitFor(() =>
      expect(screen.container.querySelector('.rolling-line-count__in')).toBeNull()
    )
    await page.screenshot({
      element: screen.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/basic-tool-timeline/live-edit-780.png'
    })
    await screen.rerender(view(next, { width: 320, dark: true }))
    expect(
      editLeaf.querySelector('.file-change-activity__item-line')!.getBoundingClientRect().height
    ).toBeLessThan(40)
    const narrow = screen.container.firstElementChild as HTMLElement
    expect(narrow.scrollWidth).toBeLessThanOrEqual(narrow.clientWidth)
    await page.screenshot({
      element: narrow,
      path: '../../../../../../.cache/basic-tool-timeline/live-edit-dark-320.png'
    })
    await screen.rerender(
      view({
        ...next,
        status: 'completed',
        fileChanges: [{ ...snapshot, additions: 33, status: 'applied', statsFinal: true }]
      })
    )
    expect(top.textContent).not.toContain('正在')
    expect(top.querySelector('.agent-running-text')).toBeNull()
    await page.screenshot({
      element: screen.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/basic-tool-timeline/completed-780.png'
    })
  })

  it('keeps long details on one line in light and dark narrow panels with keyboard disclosure', async () => {
    await page.viewport(1060, 860)
    const read = call('read', 'read_file', {
      path: '03_Cases/C001_FRP_Bimolecular_Termination_CSTR/applicability_assessment.md'
    })
    const command = call('check', 'run_command', {
      command: 'python3 validation/check.py',
      reason: '检查修改后的报告与验证记录，并复核所有行内公式及单位格式'
    })
    const search = call('search', 'search_code', { query: 'TODO', path: 'src' })
    const initial = run([read, command, search])
    for (const dark of [false, true]) {
      const screen = await render(view(initial, { width: 320, dark, mode: 'observer' }))
      const button = header(screen.container)
      button.focus()
      await userEvent.keyboard('{Enter}')
      expect(button.getAttribute('aria-expanded')).toBe('true')
      const panel = screen.container.querySelector<HTMLElement>('.basic-tool-test-panel')!
      expect(panel.scrollWidth).toBeLessThanOrEqual(panel.clientWidth)
      for (const row of panel.querySelectorAll('.agent-activity__static-summary, summary')) {
        expect(row.getBoundingClientRect().height).toBeLessThan(40)
      }
      expect(panel.querySelector('.read-activity__text-item')?.getAttribute('title')).toBe(
        '03_Cases/C001_FRP_Bimolecular_Termination_CSTR/applicability_assessment.md'
      )
      await page.screenshot({
        element: panel,
        path: `../../../../../../.cache/basic-tool-timeline/${dark ? 'dark' : 'light'}-320.png`
      })
      await screen.unmount()
    }
  })
})
