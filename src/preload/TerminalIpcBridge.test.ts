import { expect, it, vi } from 'vitest'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createTerminalIpcBridge } from './TerminalIpcBridge'

it('forwards folder identifiers and real-input provenance while preserving input order and Unicode chunks', async () => {
  const ipc = { invoke: vi.fn().mockResolvedValue(undefined), on: vi.fn(), send: vi.fn() }
  const bridge = createTerminalIpcBridge(ipc)
  await bridge.selectSourceDirectory('session', 'folder')
  expect(ipc.invoke).toHaveBeenCalledExactlyOnceWith(
    HOST_CHANNELS.terminal.selectSourceDirectory,
    'session',
    'folder'
  )
  bridge.markUserInput('session')
  bridge.writeInput('session', '\x1b[1;1R', false)
  const text = 'a'.repeat(64 * 1024 - 1) + '😀' + 'b'
  bridge.writeInput('session', text)
  const calls = ipc.send.mock.calls
  expect(calls[0]).toEqual([HOST_CHANNELS.terminal.markUserInput, 'session'])
  expect(calls[1]).toEqual([HOST_CHANNELS.terminal.writeInput, 'session', '\x1b[1;1R', false])
  expect(
    calls
      .slice(2)
      .map(([, , data]) => data)
      .join('')
  ).toBe(text)
  expect(calls.slice(2).every(([, , , userInitiated]) => userInitiated === true)).toBe(true)
  expect(calls[2][2].endsWith('a')).toBe(true)
  expect(calls[3][2]).toBe('😀b')
})
