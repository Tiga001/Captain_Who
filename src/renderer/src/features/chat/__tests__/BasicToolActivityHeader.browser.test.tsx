import type {
  AgentFileChangeOperation,
  AgentFileChangeSnapshot,
  AgentToolCall
} from '@mycopilot/protocol'
import { expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getTranslation } from '../../../config/frontendTranslations'
import { ensureAgentRun } from '../../agentRun/agentEventReducer'
import { BasicToolActivityHeader } from '../components/toolActivities/BasicToolActivityHeader'
import type { BasicToolItem } from '../components/basicToolTimeline'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('en-US', key)
  })
}))

function editFixture(operation: AgentFileChangeOperation, ordinal = 0) {
  const call: AgentToolCall = {
    id: operation,
    tool: 'apply_patch',
    args: { request: { action: 'begin', transactionId: operation } },
    approvalStatus: 'not_required',
    reason: null
  }
  const transaction: AgentFileChangeSnapshot = {
    schemaVersion: 1,
    transactionId: operation,
    conversationId: 'conversation',
    projectId: null,
    filePath: `src/${operation}.txt`,
    operation,
    updateStrategy: 'modify',
    status: 'applying',
    baseRevision: null,
    additions: operation === 'delete' ? 0 : 3,
    deletions: operation === 'create' ? 0 : 1,
    lineCount: 5,
    byteCount: 20,
    mutationCount: 1,
    nextMutationIndex: 1,
    statsFinal: false,
    summary: null,
    createdAt: 1,
    updatedAt: 2
  }
  const item: BasicToolItem = {
    id: operation,
    callIds: [operation],
    firstOrdinal: ordinal,
    latestOrdinal: ordinal,
    category: 'edit'
  }
  return { call, transaction, item }
}

it('keeps the active action and settled categories in the header without issue labels', async () => {
  const run = ensureAgentRun(undefined, 'run', 'running')
  run.toolCalls = [
    {
      id: 'read',
      tool: 'read_file',
      args: { path: 'src/example.ts' },
      approvalStatus: 'not_required',
      reason: null
    },
    {
      id: 'command',
      tool: 'run_command',
      args: { command: 'private executable body', reason: 'Check the project' },
      approvalStatus: 'not_required',
      reason: null
    }
  ]
  const items: BasicToolItem[] = [
    { id: 'read', callIds: ['read'], firstOrdinal: 0, latestOrdinal: 0, category: 'read' },
    { id: 'command', callIds: ['command'], firstOrdinal: 1, latestOrdinal: 1, category: 'command' }
  ]
  const onToggle = vi.fn()
  const screen = await render(
    <BasicToolActivityHeader
      run={run}
      items={items}
      expanded={false}
      controls="basic-details"
      onToggle={onToggle}
    />
  )
  expect(screen.container.textContent).toContain('Running Check the project')
  expect(screen.container.textContent).not.toContain('private executable body')
  const button = screen.getByRole('button')
  await button.click()
  expect(onToggle).toHaveBeenCalledOnce()
  expect(button.element().getAttribute('aria-controls')).toBe('basic-details')
  run.toolResults = [{ callId: 'command', tool: 'run_command', ok: false }]
  await screen.rerender(
    <BasicToolActivityHeader
      run={run}
      items={items}
      expanded
      controls="basic-details"
      onToggle={onToggle}
    />
  )
  expect(button.element().textContent).toBe('Reading example.ts')
  expect(button.element().getAttribute('title')).toBe('Reading src/example.ts')
  expect(screen.container.querySelector('.basic-tool-activity__attention')).toBeNull()
  expect(screen.container.querySelector('[aria-label]')).toBeNull()
  expect(button.element().getAttribute('aria-expanded')).toBe('true')
  run.toolResults.push({ callId: 'read', tool: 'read_file', ok: true })
  await screen.rerender(
    <BasicToolActivityHeader
      run={run}
      items={items}
      expanded
      controls="basic-details"
      onToggle={onToggle}
    />
  )
  expect(button.element().textContent).toBe('File reads: 1 · Commands: 1')
  expect(button.element().getAttribute('title')).toBe('File reads: 1 · Commands: 1')
  expect(screen.container.querySelector('.basic-tool-activity__attention')).toBeNull()
  expect(screen.container.querySelector('[aria-label]')).toBeNull()
  expect(screen.container.querySelector('.agent-running-text')).toBeNull()
})

it.each([
  ['create', 'Creating'],
  ['update', 'Editing'],
  ['delete', 'Deleting']
] as const)(
  'uses the %s action while retaining approval and ready semantics',
  async (operation, action) => {
    const run = ensureAgentRun(undefined, 'run', 'running')
    const edit = editFixture(operation)
    run.toolCalls = [edit.call]
    run.fileChanges = [edit.transaction]
    const header = () => (
      <BasicToolActivityHeader
        run={run}
        items={[edit.item]}
        expanded={false}
        controls="basic-details"
        onToggle={vi.fn()}
      />
    )
    const screen = await render(header())
    const button = screen.getByRole('button').element()
    const label = () => screen.container.querySelector('.basic-tool-activity__label')
    expect(label()?.textContent).toBe(`${action} ${operation}.txt`)
    expect(button.getAttribute('title')).toBe(`${action} src/${operation}.txt`)
    expect(button.getAttribute('data-status')).toBe('running')
    expect(button.getAttribute('data-active')).toBe('true')
    expect(label()?.classList.contains('agent-running-text')).toBe(true)

    edit.transaction.status = 'waiting_approval'
    await screen.rerender(header())
    expect(label()?.textContent).toBe(`Awaiting approval ${operation}.txt`)
    expect(button.getAttribute('data-status')).toBe('awaiting_approval')
    expect(button.getAttribute('data-active')).toBe('false')
    expect(label()?.classList.contains('agent-running-text')).toBe(false)

    edit.transaction.status = 'ready'
    await screen.rerender(header())
    expect(label()?.textContent).toBe(`Ready to apply ${operation}.txt`)
    expect(button.getAttribute('data-status')).toBe('ready')
    expect(button.getAttribute('data-active')).toBe('false')
    expect(label()?.classList.contains('agent-running-text')).toBe(false)
  }
)

it('summarizes completed operations separately and counts a staged create only once', async () => {
  const run = ensureAgentRun(undefined, 'run', 'completed')
  const created = editFixture('create')
  const updated = editFixture('update', 3)
  const deleted = editFixture('delete', 4)
  run.toolCalls = [
    created.call,
    {
      ...created.call,
      id: 'append-create',
      args: { request: { action: 'append', transactionId: 'create', content: 'new content' } }
    },
    {
      ...created.call,
      id: 'commit-create',
      args: { request: { action: 'commit', transactionId: 'create' } }
    },
    updated.call,
    deleted.call
  ]
  run.fileChanges = [created.transaction, updated.transaction, deleted.transaction].map(
    (transaction) => ({ ...transaction, status: 'applied' })
  )
  const screen = await render(
    <BasicToolActivityHeader
      run={run}
      items={[
        {
          ...created.item,
          callIds: ['create', 'append-create', 'commit-create'],
          latestOrdinal: 2
        },
        updated.item,
        deleted.item
      ]}
      expanded={false}
      controls="basic-details"
      onToggle={vi.fn()}
    />
  )
  const button = screen.getByRole('button').element()
  expect(button.textContent).toBe('Edited 1 files · Created 1 files · Deleted 1 files')
  expect(button.getAttribute('data-status')).toBe('completed')
  expect(screen.container.querySelector('.agent-running-text')).toBeNull()
  expect(screen.container.querySelector('.basic-tool-activity__counts')).toBeNull()
})
