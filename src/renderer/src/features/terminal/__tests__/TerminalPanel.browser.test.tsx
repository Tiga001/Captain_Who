import type { ReactNode } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import type {
  TerminalCreateSessionRequest,
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
  subscribeSession,
  writeInput
} = vi.hoisted(() => ({
  acknowledgeOutput: vi.fn<(sessionId: string, sequence: number) => void>(),
  createSession:
    vi.fn<(request: TerminalCreateSessionRequest) => Promise<TerminalSessionSnapshot>>(),
  killSession: vi.fn<(sessionId: string) => Promise<boolean>>(),
  resizeSession: vi.fn<(sessionId: string, cols: number, rows: number) => Promise<void>>(),
  subscribeSession: vi.fn<(sessionId: string, handlers: SessionHandlers) => () => void>(),
  writeInput: vi.fn<(sessionId: string, data: string) => void>()
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
  createSession.mockReset().mockImplementation(async (request) => sessionSnapshot(request))
  killSession.mockReset().mockResolvedValue(true)
  resizeSession.mockReset().mockResolvedValue(undefined)
  writeInput.mockReset()
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

function sessionSnapshot(request: TerminalCreateSessionRequest): TerminalSessionSnapshot {
  if (!request.sessionId) throw new Error('Terminal must subscribe with an ID before creation')
  return {
    cols: request.cols ?? 80,
    cwd: request.cwd ?? '/home/test',
    rows: request.rows ?? 24,
    sessionId: request.sessionId,
    shell: '/bin/sh'
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
    expect(initial.rows).toBeGreaterThanOrEqual(2)
    expect(initial.rows).toBeLessThan(8)
    const container = screen.container.querySelector<HTMLElement>('.terminal-panel__xterm')!
    expect(container.getBoundingClientRect().height).toBe(111)

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
    let resolvePending!: (snapshot: TerminalSessionSnapshot) => void
    const pending = new Promise<TerminalSessionSnapshot>((resolve) => {
      resolvePending = resolve
    })
    createSession.mockImplementation((request) =>
      request.cwd === '/repo/a' ? pending : Promise.resolve(sessionSnapshot(request))
    )
    const tabs = [...INITIAL_TABS, { cwd: '/repo/b', id: 'tab-b' }]
    const screen = await render(<TerminalTabsFixture tabs={tabs} />)
    await expect.poll(() => createSession.mock.calls.length).toBe(2)
    const firstRequest = requestForCwd('/repo/a')
    const second = requestForCwd('/repo/b').sessionId

    await screen.rerender(<TerminalTabsFixture activeId="tab-b" tabs={[tabs[1]]} />)

    expect(killSession).toHaveBeenCalledExactlyOnceWith(firstRequest.sessionId)
    expect(subscriptions.get(firstRequest.sessionId)?.unsubscribe).toHaveBeenCalledOnce()
    resolvePending(sessionSnapshot(firstRequest))
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
