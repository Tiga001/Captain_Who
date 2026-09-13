import { page, userEvent } from 'vitest/browser'
import type { AppProjectFolder } from '../../../config/projectConfig'
import { StrictMode, type ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import type {
  TerminalCreateSessionRequest,
  TerminalCreateSessionResult,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '../terminalTypes'
import '../../../styles/global.css'
import '../../bottomPanel/BottomPanel.css'

interface SessionHandlers {
  onExit: (event: TerminalExitEvent) => void
  onOutput: (event: TerminalOutputEvent) => void
}

const {
  acknowledgeOutput,
  createSession,
  killSession,
  resizeSession,
  markUserInput,
  selectSourceDirectory,
  subscribeSession,
  writeInput
} = vi.hoisted(() => ({
  markUserInput: vi.fn<(sessionId: string) => void>(),
  selectSourceDirectory: vi.fn<(sessionId: string, folderId: string) => Promise<void>>(),
  acknowledgeOutput: vi.fn<(sessionId: string, sequence: number) => void>(),
  createSession:
    vi.fn<(request: TerminalCreateSessionRequest) => Promise<TerminalCreateSessionResult>>(),
  killSession: vi.fn<(sessionId: string) => Promise<boolean>>(),
  resizeSession: vi.fn<(sessionId: string, cols: number, rows: number) => Promise<void>>(),
  subscribeSession: vi.fn<(sessionId: string, handlers: SessionHandlers) => () => void>(),
  writeInput: vi.fn<(sessionId: string, data: string, userInitiated?: boolean) => void>()
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    resolvedThemeId: 'classic-light',
    t: (key: string) => key
  })
}))

vi.mock('../terminalClient', () => ({
  acknowledgeTerminalOutput: acknowledgeOutput,
  createTerminalSession: createSession,
  killTerminalSession: killSession,
  resizeTerminalSession: resizeSession,
  subscribeTerminalSession: subscribeSession,
  markTerminalUserInput: markUserInput,
  selectTerminalSourceDirectory: selectSourceDirectory,
  writeTerminalInput: writeInput
}))

const { TerminalPanel } = await import('../TerminalPanel')
const themeVariables = getFrontendCssVariables()
const subscriptions = new Map<
  string,
  { handlers: SessionHandlers; unsubscribe: ReturnType<typeof vi.fn> }
>()

beforeEach(() => {
  subscriptions.clear()
  acknowledgeOutput.mockReset()
  createSession.mockReset().mockImplementation(async (request) => created(sessionSnapshot(request)))
  killSession.mockReset().mockResolvedValue(true)
  resizeSession.mockReset().mockResolvedValue(undefined)
  writeInput.mockReset()
  markUserInput.mockReset()
  selectSourceDirectory.mockReset().mockResolvedValue(undefined)
  subscribeSession.mockReset().mockImplementation((sessionId, handlers) => {
    const unsubscribe = vi.fn()
    subscriptions.set(sessionId, { handlers, unsubscribe })
    return unsubscribe
  })
  for (const [name, value] of Object.entries(themeVariables)) {
    document.documentElement.style.setProperty(name, value)
  }
})

afterEach(async () => {
  await cleanup()
  for (const name of Object.keys(themeVariables)) {
    document.documentElement.style.removeProperty(name)
  }
})

interface TestTab {
  cwd: string
  id: string
}

const INITIAL_TABS: TestTab[] = [{ cwd: '/repo/a', id: 'tab-a' }]

function TerminalTabsFixture({
  activeId = 'tab-a',
  height = 280,
  shown = true,
  tabs = INITIAL_TABS,
  width = 500
}: {
  activeId?: string
  height?: number
  shown?: boolean
  tabs?: TestTab[]
  width?: number
}): ReactNode {
  return (
    <div hidden={!shown} style={{ height, position: 'relative', width }}>
      {tabs.map((tab) => {
        const active = shown && tab.id === activeId
        return (
          <div
            data-testid={tab.id}
            key={tab.id}
            style={{
              contentVisibility: active ? 'visible' : 'hidden',
              inset: 0,
              pointerEvents: active ? 'auto' : 'none',
              position: 'absolute',
              visibility: active ? 'visible' : 'hidden'
            }}
          >
            <TerminalPanel initialCwd={tab.cwd} isActive={active} />
          </div>
        )
      })}
    </div>
  )
}

function CompactBottomTerminal({ height }: { height: number }): ReactNode {
  return (
    <div className="bottom-panel" style={{ height, width: 500 }}>
      <div className="bottom-panel__toolbar">Terminal</div>
      <TerminalPanel initialCwd="/repo/a" isActive />
    </div>
  )
}

function created(session: TerminalSessionSnapshot): TerminalCreateSessionResult {
  return { status: 'created', session }
}

function sessionSnapshot(request: TerminalCreateSessionRequest): TerminalSessionSnapshot {
  if (!request.sessionId) throw new Error('Terminal must subscribe with an ID before creation')
  return {
    cols: request.cols ?? 80,
    cwd: request.cwd ?? '/home/test',
    rows: request.rows ?? 24,
    sessionId: request.sessionId,
    shell: '/bin/sh',
    sourceFolders:
      request.projectId === 'project-a' ? sourceFolders.map((folder) => ({ ...folder })) : []
  }
}

function requestForCwd(cwd: string): TerminalCreateSessionRequest & { sessionId: string } {
  const request = createSession.mock.calls.find(([candidate]) => candidate.cwd === cwd)?.[0]
  if (!request?.sessionId) throw new Error(`Terminal session was not created for ${cwd}`)
  return { ...request, sessionId: request.sessionId }
}

function emitOutput(sessionId: string, sequence: number, data: string): void {
  const subscription = subscriptions.get(sessionId)
  if (!subscription || subscription.unsubscribe.mock.calls.length > 0) {
    throw new Error(`Terminal output subscription is not active: ${sessionId}`)
  }
  subscription.handlers.onOutput({ data, sequence, sessionId })
}

describe('TerminalPanel session lifetime', () => {
  it('fits the compact 165px bottom panel on creation and after resizing back down', async () => {
    const screen = await render(<CompactBottomTerminal height={165} />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const initial = requestForCwd('/repo/a')
    expect(initial).not.toHaveProperty('projectId')
    expect(initial.rows).toBeGreaterThanOrEqual(2)
    expect(initial.rows).toBeLessThan(8)
    const container = screen.container.querySelector<HTMLElement>('.terminal-panel__xterm')!
    const toolbarHeight = screen.container
      .querySelector('.bottom-panel__toolbar')!
      .getBoundingClientRect().height
    expect(container.getBoundingClientRect().height).toBe(165 - toolbarHeight - 8)
    expect(screen.container.querySelector('.terminal-panel__status')).toBeNull()

    await screen.rerender(<CompactBottomTerminal height={280} />)
    await expect.poll(() => resizeSession.mock.calls.at(-1)?.[2]).toBeGreaterThan(initial.rows!)
    await screen.rerender(<CompactBottomTerminal height={165} />)
    await expect.poll(() => resizeSession.mock.calls.at(-1)?.[2]).toBe(initial.rows)
    expect(
      screen.container.querySelector('.xterm-screen')!.getBoundingClientRect().bottom
    ).toBeLessThanOrEqual(container.getBoundingClientRect().bottom + 1)
    expect(createSession).toHaveBeenCalledOnce()
    expect(killSession).not.toHaveBeenCalled()
  })

  it('keeps the real xterm and output ACKs alive while hidden, then shows buffered output', async () => {
    const screen = await render(<TerminalTabsFixture />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const { sessionId } = requestForCwd('/repo/a')
    const xterm = screen.getByTestId('tab-a').element().querySelector('.xterm')
    expect(xterm).not.toBeNull()
    expect(subscribeSession).toHaveBeenCalledBefore(createSession)

    emitOutput(sessionId, 1, 'visible output\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([sessionId, 1])
    await screen.rerender(<TerminalTabsFixture shown={false} />)

    emitOutput(sessionId, 2, 'output while hidden\r\n')
    emitOutput(sessionId, 3, 'more hidden output\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([sessionId, 3])
    expect(createSession).toHaveBeenCalledTimes(1)
    expect(killSession).not.toHaveBeenCalled()
    expect(subscriptions.get(sessionId)?.unsubscribe).not.toHaveBeenCalled()

    await screen.rerender(<TerminalTabsFixture />)

    expect(screen.getByTestId('tab-a').element().querySelector('.xterm')).toBe(xterm)
    await expect
      .poll(() => screen.getByTestId('tab-a').element().querySelector('.xterm-rows')?.textContent)
      .toContain('output while hidden')
    expect(createSession).toHaveBeenCalledTimes(1)
    expect(killSession).not.toHaveBeenCalled()
  })

  it('retains two tab sessions across selection and only disposes the closed tab', async () => {
    const tabs = [...INITIAL_TABS, { cwd: '/repo/b', id: 'tab-b' }]
    const screen = await render(<TerminalTabsFixture tabs={tabs} />)
    await expect.poll(() => createSession.mock.calls.length).toBe(2)
    const first = requestForCwd('/repo/a').sessionId
    const second = requestForCwd('/repo/b').sessionId
    const secondXterm = screen.getByTestId('tab-b').element().querySelector('.xterm')

    emitOutput(second, 1, 'inactive second tab\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([second, 1])
    await screen.rerender(<TerminalTabsFixture activeId="tab-b" tabs={tabs} />)
    emitOutput(first, 1, 'inactive first tab\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([first, 1])
    await screen.rerender(<TerminalTabsFixture tabs={tabs} />)
    expect(createSession).toHaveBeenCalledTimes(2)
    expect(killSession).not.toHaveBeenCalled()
    expect(subscriptions.get(first)?.unsubscribe).not.toHaveBeenCalled()
    expect(subscriptions.get(second)?.unsubscribe).not.toHaveBeenCalled()

    await screen.rerender(<TerminalTabsFixture activeId="tab-b" tabs={[tabs[1]]} />)

    expect(killSession).toHaveBeenCalledExactlyOnceWith(first)
    expect(subscriptions.get(first)?.unsubscribe).toHaveBeenCalledOnce()
    expect(subscriptions.get(second)?.unsubscribe).not.toHaveBeenCalled()
    expect(screen.getByTestId('tab-b').element().querySelector('.xterm')).toBe(secondXterm)
    emitOutput(second, 2, 'surviving second tab\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([second, 2])

    await screen.unmount()

    expect(killSession.mock.calls).toEqual([[first], [second]])
    expect(subscriptions.get(second)?.unsubscribe).toHaveBeenCalledOnce()
  })

  it('disposes a closed pending session again when its late create response arrives', async () => {
    let resolvePending!: (result: TerminalCreateSessionResult) => void
    const pending = new Promise<TerminalCreateSessionResult>((resolve) => {
      resolvePending = resolve
    })
    createSession.mockImplementation((request) =>
      request.cwd === '/repo/a' ? pending : Promise.resolve(created(sessionSnapshot(request)))
    )
    const tabs = [...INITIAL_TABS, { cwd: '/repo/b', id: 'tab-b' }]
    const screen = await render(<TerminalTabsFixture tabs={tabs} />)
    await expect.poll(() => createSession.mock.calls.length).toBe(2)
    const firstRequest = requestForCwd('/repo/a')
    const second = requestForCwd('/repo/b').sessionId

    await screen.rerender(<TerminalTabsFixture activeId="tab-b" tabs={[tabs[1]]} />)

    expect(killSession).toHaveBeenCalledExactlyOnceWith(firstRequest.sessionId)
    expect(subscriptions.get(firstRequest.sessionId)?.unsubscribe).toHaveBeenCalledOnce()
    resolvePending(created(sessionSnapshot(firstRequest)))
    await expect
      .poll(() => killSession.mock.calls)
      .toEqual([[firstRequest.sessionId], [firstRequest.sessionId]])
    expect(subscriptions.get(second)?.unsubscribe).not.toHaveBeenCalled()
    emitOutput(second, 1, 'other session is unaffected\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([second, 1])
    expect(createSession).toHaveBeenCalledTimes(2)
  })

  it('pins a mounted session to its initial cwd while new tabs use the next project', async () => {
    const screen = await render(<TerminalTabsFixture />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const first = requestForCwd('/repo/a').sessionId
    const reboundTabs = [{ cwd: '/repo/b', id: 'tab-a' }]

    await screen.rerender(<TerminalTabsFixture tabs={reboundTabs} />)

    expect(createSession).toHaveBeenCalledTimes(1)
    expect(killSession).not.toHaveBeenCalled()
    expect(subscriptions.get(first)?.unsubscribe).not.toHaveBeenCalled()

    await screen.rerender(
      <TerminalTabsFixture
        activeId="tab-b"
        tabs={[...reboundTabs, { cwd: '/repo/b', id: 'tab-b' }]}
      />
    )

    await expect.poll(() => createSession.mock.calls.length).toBe(2)
    const second = requestForCwd('/repo/b').sessionId
    expect(second).not.toBe(first)
    expect(createSession.mock.calls.map(([request]) => request.cwd)).toEqual(['/repo/a', '/repo/b'])
    expect(killSession).not.toHaveBeenCalled()
  })

  it('defers resizing a hidden terminal and fits its larger container after showing it', async () => {
    const screen = await render(<TerminalTabsFixture />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const initial = requestForCwd('/repo/a')
    await screen.rerender(<TerminalTabsFixture height={440} shown={false} width={850} />)
    // Allow the ResizeObserver settle period to elapse while the container has no visible size.
    await new Promise((resolve) => window.setTimeout(resolve, 180))
    expect(resizeSession).not.toHaveBeenCalled()

    await screen.rerender(<TerminalTabsFixture height={440} width={850} />)

    await expect
      .poll(() =>
        resizeSession.mock.calls.some(
          ([sessionId, cols, rows]) =>
            sessionId === initial.sessionId && cols > initial.cols! && rows > initial.rows!
        )
      )
      .toBe(true)
    expect(createSession).toHaveBeenCalledTimes(1)
    expect(killSession).not.toHaveBeenCalled()
    expect(subscriptions.get(initial.sessionId)?.unsubscribe).not.toHaveBeenCalled()
  })
})

const sourceFolders: AppProjectFolder[] = [
  {
    id: 'primary',
    alias: 'frontend',
    path: '/repo/a',
    role: 'primary',
    sortOrder: 0,
    createdAt: 1
  },
  {
    id: 'auxiliary',
    alias: 'backend',
    path: '/repo/b',
    role: 'auxiliary',
    sortOrder: 1,
    createdAt: 1
  }
]
function SourceTerminal({ active = true }: { active?: boolean }) {
  return (
    <div style={{ height: 280, width: 600 }}>
      <TerminalPanel projectId="project-a" initialCwd="/repo/a" isActive={active} />
    </div>
  )
}

describe('TerminalPanel source directories', () => {
  it('shows multiple aliases and paths, sends one selection and retains the same session', async () => {
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const { sessionId } = requestForCwd('/repo/a')
    const button = screen.getByRole('button', { name: /backend/ })
    expect(button.element().getAttribute('title')).toBeNull()
    await button.hover()
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('/repo/b')
    expect(createSession.mock.calls[0][0].projectId).toBe('project-a')
    emitOutput(sessionId, 1, 'developer@machine frontend % ')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([sessionId, 1])
    await page.screenshot({
      element: screen.container.querySelector('.terminal-panel')!,
      path: '.vitest-attachments/terminal-source-directories.png'
    })
    const xterm = screen.container.querySelector('.xterm')
    await button.click()
    expect(selectSourceDirectory).toHaveBeenCalledExactlyOnceWith(sessionId, 'auxiliary')
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    await screen.rerender(<SourceTerminal active={false} />)
    await screen.rerender(<SourceTerminal />)
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    expect(screen.container.querySelector('.xterm')).toBe(xterm)
    expect(createSession).toHaveBeenCalledOnce()
    expect(killSession).not.toHaveBeenCalled()
  })

  it('keeps the existing layout when only one source is configured', async () => {
    createSession.mockImplementationOnce(async (request) =>
      created({
        ...sessionSnapshot(request),
        sourceFolders: sourceFolders.slice(0, 1)
      })
    )
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
  })

  it('ignores automatic DSR and focus replies, but hides permanently on the first actual key', async () => {
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const { sessionId } = requestForCwd('/repo/a')
    emitOutput(sessionId, 1, '\x1b[6n\x1b[?1004h')
    await expect.poll(() => writeInput.mock.calls.some(([, data]) => /R$/.test(data))).toBe(true)
    expect(writeInput.mock.calls.every(([, , initiated]) => initiated === false)).toBe(true)
    expect(markUserInput).not.toHaveBeenCalled()
    expect(screen.container.querySelector('.terminal-panel__sources')).not.toBeNull()
    await userEvent.keyboard('a')
    await expect.poll(() => screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    expect(markUserInput).toHaveBeenCalledExactlyOnceWith(sessionId)
    expect(writeInput.mock.calls).toContainEqual([sessionId, 'a', true])
    await screen.rerender(<SourceTerminal active={false} />)
    await screen.rerender(<SourceTerminal />)
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
  })

  it('uses physical Option/Alt digit keys without forwarding the shortcut to the shell', async () => {
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const textarea = screen.container.querySelector('textarea')!
    textarea.dispatchEvent(
      new KeyboardEvent('keydown', {
        key: '™',
        code: 'Digit2',
        altKey: true,
        bubbles: true,
        cancelable: true
      })
    )
    expect(selectSourceDirectory).toHaveBeenCalledExactlyOnceWith(
      requestForCwd('/repo/a').sessionId,
      'auxiliary'
    )
    expect(markUserInput).not.toHaveBeenCalled()
    expect(writeInput).not.toHaveBeenCalled()
  })

  it.each(['paste', 'compositionupdate'])(
    'recognizes %s as first user input',
    async (eventType) => {
      const screen = await render(<SourceTerminal />)
      await expect.poll(() => createSession.mock.calls.length).toBe(1)
      const textarea = screen.container.querySelector('textarea')!
      if (eventType === 'paste') {
        const data = new DataTransfer()
        data.setData('text/plain', 'pasted text')
        textarea.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true }))
      } else {
        textarea.dispatchEvent(
          new CompositionEvent('compositionupdate', { data: '中文', bubbles: true })
        )
      }
      await expect.poll(() => screen.container.querySelector('.terminal-panel__sources')).toBeNull()
      expect(markUserInput).toHaveBeenCalledOnce()
    }
  )

  it('reports failed selection without changing session identity or restoring source choices', async () => {
    selectSourceDirectory.mockRejectedValueOnce(new Error('Directory identity changed'))
    const screen = await render(<SourceTerminal />)
    await screen.getByRole('button', { name: /backend/ }).click()
    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('terminal.sourceDirectoryChangeFailed')
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    expect(createSession).toHaveBeenCalledOnce()
    expect(killSession).not.toHaveBeenCalled()
  })
})

describe('TerminalPanel input ordering', () => {
  it('queues real input typed before Host finishes creating the terminal', async () => {
    let finish!: (result: TerminalCreateSessionResult) => void
    createSession.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const request = requestForCwd('/repo/a')
    screen.container.querySelector<HTMLTextAreaElement>('textarea')!.focus()
    await userEvent.keyboard('x')
    expect(writeInput).not.toHaveBeenCalled()
    expect(markUserInput).not.toHaveBeenCalled()
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    finish(created(sessionSnapshot(request)))
    await expect.poll(() => writeInput.mock.calls).toContainEqual([request.sessionId, 'x', true])
    expect(markUserInput).toHaveBeenCalledExactlyOnceWith(request.sessionId)
  })

  it('sends subsequent typing after directory selection without waiting or losing keystrokes', async () => {
    let finish!: () => void
    selectSourceDirectory.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        })
    )
    const screen = await render(<SourceTerminal />)
    await screen.getByRole('button', { name: /backend/ }).click()
    await userEvent.keyboard('pwd')
    expect(writeInput.mock.calls.map(([, data]) => data).join('')).toBe('pwd')
    expect(selectSourceDirectory).toHaveBeenCalledBefore(writeInput)
    finish()
    expect(createSession).toHaveBeenCalledOnce()
  })
})

it('uses the Host-frozen source labels, paths and shortcut order after a delayed project load', async () => {
  let finish!: (result: TerminalCreateSessionResult) => void
  createSession.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      })
  )
  const screen = await render(<SourceTerminal />)
  await expect.poll(() => createSession.mock.calls.length).toBe(1)
  const request = requestForCwd('/repo/a')
  // The original project directory is now stale: Host changed a path/alias and membership
  // while loading. No old labels or folder ids should be actionable during startup.
  expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
  finish(
    created({
      ...sessionSnapshot(request),
      cwd: '/new/main',
      sourceFolders: [
        { id: 'primary', alias: 'main-now', path: '/new/main', role: 'primary' },
        { id: 'added', alias: 'added-now', path: '/new/added', role: 'auxiliary' },
        { id: 'auxiliary', alias: 'backend-now', path: '/new/backend', role: 'auxiliary' }
      ]
    })
  )
  const updated = screen.getByRole('button', { name: /backend-now/ })
  await expect.element(updated).toBeVisible()
  expect(updated.element().getAttribute('title')).toBeNull()
  await updated.hover()
  await expect.element(page.getByRole('tooltip')).toHaveTextContent('/new/backend')
  const textarea = screen.container.querySelector('textarea')!
  textarea.dispatchEvent(
    new KeyboardEvent('keydown', {
      key: '™',
      code: 'Digit2',
      altKey: true,
      bubbles: true,
      cancelable: true
    })
  )
  expect(selectSourceDirectory).toHaveBeenCalledExactlyOnceWith(request.sessionId, 'added')
  expect(createSession).toHaveBeenCalledOnce()
})

function StrictSourceTerminal({ height = 280, width = 600 }: { height?: number; width?: number }) {
  return (
    <div style={{ height, width }}>
      <TerminalPanel projectId="project-a" initialCwd="/repo/a" isActive />
    </div>
  )
}

describe('TerminalPanel StrictMode lifecycle', () => {
  it('keeps B input, source selection, resize and cleanup intact when disposed A succeeds after B', async () => {
    let finishOld!: (result: TerminalCreateSessionResult) => void
    createSession.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishOld = resolve
        })
    )
    const screen = await render(
      <StrictMode>
        <StrictSourceTerminal />
      </StrictMode>
    )
    await expect.poll(() => createSession.mock.calls.length).toBe(2)
    const previous = createSession.mock.calls[0][0]
    const current = createSession.mock.calls[1][0]
    const oldId = previous.sessionId!
    const currentId = current.sessionId!
    expect(currentId).not.toBe(oldId)
    await expect.element(screen.getByRole('button', { name: /backend/ })).toBeVisible()
    expect(killSession.mock.calls).toEqual([[oldId]])
    const currentXterm = screen.container.querySelector('.xterm')
    finishOld(
      created({
        ...sessionSnapshot(previous),
        sourceFolders: sourceFolders.map((folder) => ({
          ...folder,
          alias: `obsolete-${folder.alias}`
        }))
      })
    )
    await expect.poll(() => killSession.mock.calls).toEqual([[oldId], [oldId]])
    expect(screen.container.querySelector('.xterm')).toBe(currentXterm)
    expect(screen.container.textContent).not.toContain('obsolete-')
    await screen.getByRole('button', { name: /backend/ }).click()
    expect(selectSourceDirectory).toHaveBeenCalledExactlyOnceWith(currentId, 'auxiliary')
    await userEvent.keyboard('pwd')
    expect(writeInput.mock.calls.map(([, data]) => data).join('')).toBe('pwd')
    expect(writeInput.mock.calls.every(([sessionId]) => sessionId === currentId)).toBe(true)
    emitOutput(currentId, 1, 'current terminal still works\r\n')
    await expect.poll(() => acknowledgeOutput.mock.calls).toContainEqual([currentId, 1])
    await screen.rerender(
      <StrictMode>
        <StrictSourceTerminal height={440} width={850} />
      </StrictMode>
    )
    await expect
      .poll(() =>
        resizeSession.mock.calls.some(
          ([sessionId, cols, rows]) =>
            sessionId === currentId && cols > current.cols! && rows > current.rows!
        )
      )
      .toBe(true)
    expect(resizeSession.mock.calls.every(([sessionId]) => sessionId === currentId)).toBe(true)
    await screen.unmount()
    expect(killSession.mock.calls).toEqual([[oldId], [oldId], [currentId]])
    expect(subscriptions.get(currentId)?.unsubscribe).toHaveBeenCalledOnce()
  })

  it.each(['before', 'after'])(
    'silently discards A cancellation %s B creation completes',
    async (order) => {
      const pending: Array<(result: TerminalCreateSessionResult) => void> = []
      createSession.mockImplementation(
        () =>
          new Promise((resolve) => {
            pending.push(resolve)
          })
      )
      const screen = await render(
        <StrictMode>
          <StrictSourceTerminal />
        </StrictMode>
      )
      await expect.poll(() => pending.length).toBe(2)
      const previous = createSession.mock.calls[0][0]
      const current = createSession.mock.calls[1][0]
      if (order === 'before') pending[0]({ status: 'cancelled' })
      pending[1](created(sessionSnapshot(current)))
      await expect.element(screen.getByRole('button', { name: /backend/ })).toBeVisible()
      if (order === 'after') pending[0]({ status: 'cancelled' })
      await userEvent.keyboard('x')
      expect(writeInput.mock.calls).toContainEqual([current.sessionId, 'x', true])
      expect(screen.container.textContent).not.toContain('failed')
      expect(killSession.mock.calls).toEqual([[previous.sessionId]])
      expect(subscriptions.get(current.sessionId!)?.unsubscribe).not.toHaveBeenCalled()
      await screen.unmount()
      expect(killSession.mock.calls).toEqual([[previous.sessionId], [current.sessionId]])
    }
  )

  it('closes a current cancelled creation without displaying a startup failure', async () => {
    createSession.mockResolvedValueOnce({ status: 'cancelled' })
    const screen = await render(<SourceTerminal />)
    await expect.poll(() => createSession.mock.calls.length).toBe(1)
    const current = createSession.mock.calls[0][0]
    await expect
      .poll(() => subscriptions.get(current.sessionId!)?.unsubscribe.mock.calls.length)
      .toBe(1)
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    expect(screen.container.textContent).not.toContain('failed')
    screen.container.querySelector<HTMLTextAreaElement>('textarea')!.focus()
    await userEvent.keyboard('x')
    expect(writeInput).not.toHaveBeenCalled()
  })

  it('still displays a genuine startup failure in the current effect', async () => {
    createSession.mockRejectedValueOnce(new Error('PTY spawn failed'))
    const screen = await render(<SourceTerminal />)
    await expect
      .poll(() => screen.container.querySelector('.xterm-rows')?.textContent)
      .toContain('terminal start failed: PTY spawn failed')
    expect(screen.container.querySelector('.terminal-panel__sources')).toBeNull()
    expect(writeInput).not.toHaveBeenCalled()
  })
})

it('applies a resize that happened while creation was pending only after Host confirms the session', async () => {
  let finish!: (result: TerminalCreateSessionResult) => void
  createSession.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      })
  )
  const screen = await render(<TerminalTabsFixture />)
  await expect.poll(() => createSession.mock.calls.length).toBe(1)
  const request = requestForCwd('/repo/a')
  await screen.rerender(<TerminalTabsFixture height={440} width={850} />)
  // Let the real ResizeObserver settle update xterm while Host creation is still pending.
  await new Promise((resolve) => window.setTimeout(resolve, 200))
  expect(resizeSession).not.toHaveBeenCalled()
  finish(created(sessionSnapshot(request)))
  await expect
    .poll(() =>
      resizeSession.mock.calls.some(
        ([sessionId, cols, rows]) =>
          sessionId === request.sessionId && cols > request.cols! && rows > request.rows!
      )
    )
    .toBe(true)
})
