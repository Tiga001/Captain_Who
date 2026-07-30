import type {
  McpServerDetailsView,
  McpServerStateView,
  McpToolSummaryView
} from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const translated = vi.hoisted(() => ({
  t: (key: string) => key
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: translated.t })
}))

const [{ McpServerDetail }, { McpServerEditor }, { McpServerList }, { McpToolCatalog }] =
  await Promise.all([
    import('../McpServerDetail'),
    import('../McpServerEditor'),
    import('../McpServerList'),
    import('../McpToolCatalog')
  ])

const SERVER_ID = '11111111-1111-4111-8111-111111111111'

function server(overrides: Partial<McpServerDetailsView> = {}): McpServerDetailsView {
  return {
    schemaVersion: 1,
    serverId: SERVER_ID,
    displayName: 'fixture',
    scope: 'user',
    source: 'userManual',
    transport: 'stdio',
    trust: 'untrusted',
    approvalMode: 'prompt',
    registryRevision: 2,
    configEpoch: '22222222-2222-4222-8222-222222222222',
    configDigest: 'a'.repeat(64),
    state: 'disabled',
    enabled: false,
    launchAuthorizationState: 'required',
    catalogGeneration: 1,
    catalogCompleteness: 'complete',
    toolCount: 1,
    activeCallCount: 0,
    updatedAtMs: 2,
    executable: '/usr/bin/fixture',
    arguments: [],
    cwd: '/tmp',
    createdAtMs: 1,
    ...overrides
  }
}

describe('MCP Server editor', () => {
  it('submits exact ordered argv including empty and shell-looking values', async () => {
    const submit = vi.fn()
    const screen = await render(
      <McpServerEditor
        busy={false}
        onCancel={vi.fn()}
        onSelectExecutable={async () => null}
        onSelectWorkingDirectory={async () => null}
        onSubmit={submit}
      />
    )
    await screen.getByLabelText('mcp.form.name').fill('fixture')
    await screen.getByLabelText('mcp.form.executable').fill('/usr/bin/fixture')
    await screen.getByLabelText('mcp.form.cwd').fill('/tmp')
    await screen.getByRole('button', { name: 'mcp.form.addArgument' }).click()
    await screen.getByRole('button', { name: 'mcp.form.addArgument' }).click()
    await screen.getByLabelText('mcp.form.argument 2').fill('$(never-run); | one argv')
    await screen.getByRole('button', { name: 'mcp.actions.saveServer' }).click()
    await expect.poll(() => submit.mock.calls.length).toBe(1)
    expect(submit).toHaveBeenCalledWith({
      approvalMode: 'prompt',
      arguments: ['', '$(never-run); | one argv'],
      cwd: '/tmp',
      displayName: 'fixture',
      executable: '/usr/bin/fixture'
    })
  })

  it('does not expose unsupported remote, OAuth, environment, or allow-always controls', async () => {
    const screen = await render(
      <McpServerEditor
        busy={false}
        onCancel={vi.fn()}
        onSelectExecutable={async () => null}
        onSelectWorkingDirectory={async () => null}
        onSubmit={vi.fn()}
      />
    )
    await expect.element(screen.getByText('STDIO · mcp.form.localProcess')).toBeVisible()
    expect(screen.getByText(/HTTP/i).query()).toBeNull()
    expect(screen.getByText(/OAuth/i).query()).toBeNull()
    expect(screen.getByText(/environment/i).query()).toBeNull()
    expect(screen.getByText(/always allow/i).query()).toBeNull()
  })

  it('keeps picker values unchanged when native selection is cancelled', async () => {
    const screen = await render(
      <McpServerEditor
        busy={false}
        initial={server()}
        onCancel={vi.fn()}
        onSelectExecutable={async () => null}
        onSelectWorkingDirectory={async () => null}
        onSubmit={vi.fn()}
      />
    )
    await screen.getByRole('button', { name: 'mcp.form.chooseExecutable' }).click()
    await screen.getByRole('button', { name: 'mcp.form.chooseCwd' }).click()
    await expect
      .element(screen.getByLabelText('mcp.form.executable'))
      .toHaveValue('/usr/bin/fixture')
    await expect.element(screen.getByLabelText('mcp.form.cwd')).toHaveValue('/tmp')
  })

  it('handles a rejected native picker without an unhandled rejection', async () => {
    const screen = await render(
      <McpServerEditor
        busy={false}
        initial={server()}
        onCancel={vi.fn()}
        onSelectExecutable={async () => {
          throw new Error('fixed-picker-canary')
        }}
        onSelectWorkingDirectory={async () => null}
        onSubmit={vi.fn()}
      />
    )
    await screen.getByRole('button', { name: 'mcp.form.chooseExecutable' }).click()
    await expect.element(screen.getByText('mcp.form.pickerFailed')).toBeVisible()
    expect(screen.container.textContent).not.toContain('fixed-picker-canary')
  })

  it('preserves an unsaved draft across runtime-only Server status updates', async () => {
    const props = {
      busy: false,
      onCancel: vi.fn(),
      onSelectExecutable: async () => null,
      onSelectWorkingDirectory: async () => null,
      onSubmit: vi.fn()
    }
    const screen = await render(<McpServerEditor {...props} initial={server()} />)
    await screen.getByLabelText('mcp.form.name').fill('unsaved draft')
    await screen.rerender(
      <McpServerEditor
        {...props}
        initial={server({
          activeCallCount: 2,
          enabled: true,
          state: 'degraded',
          updatedAtMs: 3
        })}
      />
    )
    await expect.element(screen.getByLabelText('mcp.form.name')).toHaveValue('unsaved draft')
  })
})

describe('MCP Server list', () => {
  it('keeps enabled state separate from every connection state and supports duplicate names', async () => {
    const states: McpServerStateView[] = [
      'disabled',
      'starting',
      'discovering',
      'ready',
      'stopping',
      'error',
      'backoff',
      'degraded'
    ]
    const screen = await render(
      <McpServerList
        onDelete={vi.fn()}
        onOpen={vi.fn()}
        onRestart={vi.fn()}
        onSetEnabled={vi.fn()}
        pendingOperations={new Map()}
        servers={states.map((state, index) => ({
          ...server(),
          serverId: `server-${index}`,
          displayName: 'duplicate name',
          state,
          enabled: state === 'starting'
        }))}
      />
    )

    for (const state of states) {
      await expect.element(screen.getByText(`mcp.state.${state}`)).toBeVisible()
    }
    expect(screen.container.querySelectorAll('.mcp-server-row')).toHaveLength(states.length)
    expect(screen.getByText('duplicate name').elements()).toHaveLength(states.length)
    const readyRow = screen.container.querySelectorAll('.mcp-server-row')[3]
    expect(readyRow.textContent).toContain('mcp.state.ready')
    expect(readyRow.textContent).toContain('mcp.disabled')
  })
})

describe('MCP Server details', () => {
  it('fails closed for an out-of-range safe integer timestamp', async () => {
    const screen = await render(
      <McpServerDetail
        onAuthorize={vi.fn()}
        onBack={vi.fn()}
        onEdit={vi.fn()}
        onRestart={vi.fn()}
        onSetEnabled={vi.fn()}
        server={server({
          createdAtMs: Number.MAX_SAFE_INTEGER,
          updatedAtMs: Number.MAX_SAFE_INTEGER
        })}
      />
    )
    await expect.element(screen.getByText('—').first()).toBeVisible()
  })
})

describe('MCP Catalog safe presentation', () => {
  it('renders hostile Server text only as stripped plain text', async () => {
    const hostile = '<img src=x onerror=fixed-canary>\u202e\u0000safe'
    const tool: McpToolSummaryView = {
      serverId: SERVER_ID,
      rawName: hostile,
      modelName: 'model-name',
      routable: false,
      disabled: true,
      schemaDigestPrefix: 'abcdefabcdef',
      description: hostile,
      descriptionTruncated: false,
      diagnosticCodes: ['<script>fixed-canary</script>'],
      catalogGeneration: 1,
      catalogCompleteness: 'partial'
    }
    const screen = await render(
      <McpToolCatalog
        catalog={{
          catalogCompleteness: 'partial',
          catalogGeneration: 1,
          errorMessage: null,
          isRefreshing: false,
          status: 'ready',
          tools: [tool]
        }}
        onLoad={vi.fn()}
        onLoadMore={vi.fn()}
        onRefresh={vi.fn()}
        server={{ ...server(), displayName: hostile }}
      />
    )
    await expect
      .element(screen.getByText('<img src=x onerror=fixed-canary>safe').first())
      .toBeVisible()
    expect(screen.container.querySelector('img')).toBeNull()
    expect(screen.container.querySelector('script')).toBeNull()
    expect(screen.container.textContent).not.toContain('\u202e')
    expect(screen.container.textContent).not.toContain('\u0000')
  })

  it('never places a Host cursor in the DOM', async () => {
    const cursor = 'fixed-cursor-canary-must-not-cross'
    const screen = await render(
      <McpToolCatalog
        catalog={{
          catalogCompleteness: 'complete',
          catalogGeneration: 1,
          errorMessage: null,
          isRefreshing: false,
          nextCursor: cursor,
          status: 'ready',
          tools: []
        }}
        onLoad={vi.fn()}
        onLoadMore={vi.fn()}
        onRefresh={vi.fn()}
        server={server()}
      />
    )
    expect(screen.container.textContent).not.toContain(cursor)
    expect(screen.container.innerHTML).not.toContain(cursor)
  })
})
