import type { AgentToolCall } from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getTranslation, type TranslationKey } from '../../../config/frontendTranslations'
import type { ChatAgentRunView, ChatFileChangePreview } from '../chatTypes'
import type { BasicToolItem } from '../components/basicToolTimeline'
import { BasicToolActivityItem } from '../components/toolActivities/BasicToolActivityItem'

const { revealFile } = vi.hoisted(() => ({ revealFile: vi.fn().mockResolvedValue(undefined) }))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: TranslationKey) => getTranslation('zh-CN', key)
  })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../storage/storageClient', () => ({
  revealStoredProjectFile: revealFile,
  loadImageFile: vi.fn()
}))
vi.mock('../../agent/agentClient', () => ({
  getAgentFileChangeDiff: vi.fn(),
  getAgentFileChangeHistoryDiff: vi.fn()
}))
vi.mock('../components/toolActivities/FileChangeDiffCard', () => ({
  FileChangeDiffCard: ({ patch, toolCallId }: { patch: string; toolCallId: string }) => (
    <pre data-testid="patch-preview" data-call-id={toolCallId}>
      {patch}
    </pre>
  )
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))

function call(id: string, tool: string, args: unknown): AgentToolCall {
  return { id, tool, args, approvalStatus: 'not_required', reason: null }
}

function run(calls: AgentToolCall[]): ChatAgentRunView {
  return {
    runId: 'run',
    status: 'running',
    toolDefinitions: [],
    toolCalls: calls,
    toolResults: [],
    approvals: [],
    fileChangeProposals: [],
    timeline: []
  }
}

function item(callId: string, category: BasicToolItem['category']): BasicToolItem {
  return { id: `anchor-${callId}`, callIds: [callId], firstOrdinal: 0, latestOrdinal: 0, category }
}

it('keeps an expanded command and its output node when promoted to a compact row', async () => {
  const command = call('command', 'run_command', { command: 'printf hello', reason: 'Read output' })
  const state = run([command])
  state.toolResults = [
    {
      callId: command.id,
      tool: command.tool,
      ok: true,
      result: { status: 'exited', exitCode: 0, stdout: 'hello', stderr: '' }
    }
  ]
  const leaf = item(command.id, 'command')
  const expanded = vi.fn()
  const screen = await render(
    <BasicToolActivityItem run={state} item={leaf} onExpandedChange={expanded} />
  )
  const disclosure = screen.container.querySelector<HTMLDetailsElement>('details')!
  await userEvent.click(disclosure.querySelector('summary')!)
  await vi.waitFor(() => expect(expanded).toHaveBeenLastCalledWith(true))
  const output = screen.container.querySelector('.run-command-shell__output')
  await screen.rerender(
    <BasicToolActivityItem
      run={state}
      item={leaf}
      presentation="compact"
      onExpandedChange={expanded}
    />
  )
  expect(screen.container.querySelector('details')).toBe(disclosure)
  expect(disclosure.open).toBe(true)
  expect(screen.container.querySelector('.run-command-shell__output')).toBe(output)
  expect(output?.textContent).toBe('hello')
  expect(screen.container.querySelectorAll('.run-command-shell__copy')).toHaveLength(2)
})

it('keeps an expanded edit row while its latest staged call and preview advance', async () => {
  const begin = call('begin', 'apply_patch', {
    request: {
      action: 'begin',
      transactionId: 'transaction',
      filePath: 'notes.txt',
      operation: 'create'
    }
  })
  const append = call('append', 'apply_patch', {
    request: { action: 'append', transactionId: 'transaction', content: 'second line' }
  })
  const state = run([begin])
  const preview: ChatFileChangePreview = {
    schemaVersion: 1,
    previewId: 'preview',
    streamId: 'stream',
    attempt: 1,
    toolCallIndex: 0,
    toolCallId: begin.id,
    transactionId: 'transaction',
    filePath: 'notes.txt',
    additions: 1,
    deletions: 0,
    lineCount: 1,
    byteCount: 10,
    generatedBytes: 10,
    updatedAt: 1,
    receivedAt: 1,
    content: 'first line'
  }
  state.fileChangePreviews = [preview]
  const leaf = item(begin.id, 'edit')
  const expanded = vi.fn()
  const screen = await render(
    <BasicToolActivityItem run={state} item={leaf} onExpandedChange={expanded} />
  )
  const toggle = screen.container.querySelector<HTMLButtonElement>('.file-change-activity__toggle')!
  await userEvent.click(toggle)
  expect(expanded).toHaveBeenLastCalledWith(true)
  expect(toggle.getAttribute('aria-expanded')).toBe('true')
  const diff = screen.container.querySelector('[data-testid="patch-preview"]')
  await screen.rerender(
    <BasicToolActivityItem
      run={{
        ...state,
        toolCalls: [begin, append],
        fileChangePreviews: [{ ...preview, content: 'second line', receivedAt: 2 }]
      }}
      item={{ ...leaf, callIds: [begin.id, append.id], latestOrdinal: 2 }}
      presentation="compact"
      onExpandedChange={expanded}
    />
  )
  expect(screen.container.querySelector('.file-change-activity__toggle')).toBe(toggle)
  expect(toggle.getAttribute('aria-expanded')).toBe('true')
  expect(screen.container.querySelector('[data-testid="patch-preview"]')).toBe(diff)
  expect(diff?.getAttribute('data-call-id')).toBe(append.id)
  expect(diff?.textContent).toBe('second line')
  expect(screen.container.querySelector('details')).toBeNull()
})

it('shows compact file rows without a redundant category disclosure or raw history payload', async () => {
  const read = call('read', 'read_word', { path: '/workspace/report.docx' })
  const history = call('history', 'conversation_history', { query: 'private search input' })
  const state = run([read, history])
  state.toolResults = [
    { callId: history.id, tool: history.tool, ok: true, result: 'private result' }
  ]
  const screen = await render(
    <>
      <BasicToolActivityItem run={state} item={item(read.id, 'read')} presentation="compact" />
      <BasicToolActivityItem
        run={state}
        item={item(history.id, 'history')}
        presentation="compact"
      />
    </>
  )
  expect(screen.container.querySelector('.read-activity__file-name')?.textContent).toBe(
    'report.docx'
  )
  expect(screen.container.querySelector('details')).toBeNull()
  expect(screen.container.textContent).not.toContain('private')
})

it('keeps edit filenames prominent while preserving full path authority and count nodes', async () => {
  const filePath = '/workspace/reports/deeply/nested/final/report.md'
  const edit = call('edit', 'apply_patch', {
    request: { action: 'apply', operation: 'create', filePath, content: 'report\n' }
  })
  const state = run([edit])
  const leaf = item(edit.id, 'edit')
  const screen = await render(
    <BasicToolActivityItem run={state} item={leaf} assistantMessageId="answer" />
  )
  const path = screen.container.querySelector<HTMLButtonElement>('.file-change-activity__path')!
  const counts = screen.container.querySelector('.file-change-activity__stats')
  expect(path.textContent).toBe('report.md')
  expect(path.getAttribute('aria-label')).toContain(filePath)
  expect(path.title).toContain(filePath)
  expect(screen.container.querySelector('.file-change-activity__directory')).toBeNull()

  await screen.rerender(
    <BasicToolActivityItem
      run={state}
      item={leaf}
      assistantMessageId="answer"
      presentation="compact"
    />
  )
  expect(screen.container.querySelector('.file-change-activity__path')).toBe(path)
  expect(path.textContent).toBe('report.md')
  expect(screen.container.querySelector('.file-change-activity__directory')).toBeNull()
  expect(
    screen.container.querySelector('.file-change-activity__item-line')?.getAttribute('title')
  ).toBe(filePath)
  expect(screen.container.querySelector('.file-change-activity__stats')).toBe(counts)
  await userEvent.click(path)
  expect(revealFile).toHaveBeenLastCalledWith(undefined, filePath, 'answer')
})

it('retains the full accessible path when an edit has no reveal action', async () => {
  const filePath = 'docs/reports/summary.md'
  const edit = call('edit', 'apply_patch', {
    request: { action: 'apply', operation: 'update', filePath }
  })
  const screen = await render(
    <BasicToolActivityItem run={run([edit])} item={item(edit.id, 'edit')} />
  )
  const path = screen.container.querySelector<HTMLElement>('.file-change-activity__path')!
  expect(path.tagName).toBe('SPAN')
  expect(path.textContent).toBe('summary.md')
  expect(path.getAttribute('aria-hidden')).toBe('true')
  expect(
    screen.container.querySelector('.file-change-activity__accessible-path')?.textContent
  ).toBe(filePath)
  expect(path.title).toBe(filePath)
  expect(screen.container.querySelector('.file-change-activity__directory')).toBeNull()
})

it('puts the read action before the filename while retaining failures and the source path', async () => {
  const filePath = '/workspace/client/reports/annual/summary.docx'
  const read = call('read-word', 'read_word', { path: filePath })
  const state = run([read])
  const leaf = item(read.id, 'read')
  const onOpenWorkspaceReference = vi.fn()
  const navigation = {
    assistantMessageId: 'answer',
    conversationId: 'chat',
    onOpenWorkspaceReference
  }
  const screen = await render(
    <BasicToolActivityItem {...navigation} run={state} item={leaf} presentation="compact" />
  )
  const action = () => screen.container.querySelector('.read-activity__action')
  expect(action()?.textContent).toBe('正在读取')
  expect(action()?.nextElementSibling?.textContent).toBe('summary.docx')
  expect(screen.container.querySelector('.read-activity__directory')).toBeNull()
  await userEvent.click(screen.getByRole('button', { name: filePath, exact: true }))
  expect(onOpenWorkspaceReference).toHaveBeenLastCalledWith({
    source: 'read-tool',
    filePath,
    assistantMessageId: 'answer',
    conversationId: 'chat',
    callId: read.id
  })

  await screen.rerender(
    <BasicToolActivityItem
      {...navigation}
      run={{ ...state, toolResults: [{ callId: read.id, tool: read.tool, ok: true }] }}
      item={leaf}
      presentation="compact"
    />
  )
  expect(action()?.textContent).toBe('已读取')
  await screen.rerender(
    <BasicToolActivityItem
      run={{
        ...state,
        toolResults: [
          { callId: read.id, tool: read.tool, ok: false, error: 'Cannot read this file' }
        ]
      }}
      {...navigation}
      item={leaf}
      presentation="compact"
    />
  )
  expect(action()?.textContent).toBe(getTranslation('zh-CN', 'agent.read.word.failed'))
  expect(
    screen.container.querySelector('.read-activity__text-item')?.getAttribute('title')
  ).toContain(filePath)
  await userEvent.click(screen.container.querySelector('summary .read-activity__action')!)
  await expect.element(screen.getByText('Cannot read this file')).toBeVisible()
})

it('keeps search verbs before their query in both singleton and compact rows', async () => {
  const search = call('search-code', 'search_code', { query: 'TODO' })
  const state = run([search])
  const leaf = item(search.id, 'search')
  const screen = await render(<BasicToolActivityItem run={state} item={leaf} />)
  const label = () => screen.container.querySelector('.agent-activity__label')?.textContent
  expect(label()).toBe('正在查找 TODO')
  const completed: ChatAgentRunView = {
    ...state,
    toolResults: [
      { callId: search.id, tool: search.tool, ok: true, result: { query: 'TODO', matches: [] } }
    ]
  }
  await screen.rerender(
    <BasicToolActivityItem run={completed} item={leaf} presentation="compact" />
  )
  expect(label()).toBe('已查找 TODO')
  expect(screen.container.querySelector('.search-activity__query-heading')).toBeNull()
  await screen.rerender(
    <BasicToolActivityItem
      run={{ ...state, status: 'cancelled' }}
      item={leaf}
      presentation="compact"
    />
  )
  expect(label()).toBe(`${getTranslation('zh-CN', 'agent.search.code.cancelled')} TODO`)
})

it('marks workspace folder labels with a display-only trailing slash', async () => {
  const args = { focusPath: '/workspace/nested/reports/' }
  const workspace = call('workspace', 'workspace_map', args)
  const state = run([workspace])
  const leaf = item(workspace.id, 'workspace')
  const screen = await render(<BasicToolActivityItem run={state} item={leaf} />)
  const label = () => screen.container.querySelector('.agent-activity__label')?.textContent
  expect(label()).toBe('正在读取 reports/')
  await screen.rerender(
    <BasicToolActivityItem
      run={{
        ...state,
        toolResults: [
          {
            callId: workspace.id,
            tool: workspace.tool,
            ok: true,
            result: { workspace: { focusPath: '/resolved/reports' } }
          }
        ]
      }}
      item={leaf}
    />
  )
  expect(label()).toBe('已读取 reports/')
  expect(workspace.args).toEqual(args)
  expect(args.focusPath).toBe('/workspace/nested/reports/')
})
