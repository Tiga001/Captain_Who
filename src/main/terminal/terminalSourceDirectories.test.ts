import { mkdtempSync, mkdirSync, realpathSync, renameSync, rmSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { StorageProjectRecord } from '@mycopilot/protocol'
import {
  freezeTerminalProjectSources,
  terminalChangeDirectoryCommand,
  TerminalSessionInput
} from './terminalSourceDirectories'

const temporary: string[] = []
afterEach(() =>
  temporary.splice(0).forEach((path) => rmSync(path, { recursive: true, force: true }))
)
function fixture() {
  const base = realpathSync(mkdtempSync(join(tmpdir(), 'terminal-sources-')))
  temporary.push(base)
  const primary = join(base, 'primary')
  const auxiliary = join(base, "aux ' $() `x` 中文")
  mkdirSync(primary)
  mkdirSync(auxiliary)
  const project: StorageProjectRecord = {
    id: 'project-a',
    name: 'Project',
    createdAt: 1,
    folders: [
      { id: 'main', alias: 'main', path: primary, role: 'primary', sortOrder: 0, createdAt: 1 },
      { id: 'aux', alias: 'aux', path: auxiliary, role: 'auxiliary', sortOrder: 1, createdAt: 1 }
    ]
  }
  const sources = freezeTerminalProjectSources(project)!
  const write = vi.fn<(data: string) => void>()
  const input = new TerminalSessionInput(write, '/bin/zsh', sources)
  return { base, primary, auxiliary, project, sources, write, input }
}

describe('terminal project source input', () => {
  it('submits one safely quoted cd to the existing PTY then serializes subsequent input', () => {
    const { input, write, auxiliary } = fixture()
    input.writeInput('\x1b[1;1R', false)
    input.selectSourceDirectory('aux')
    input.writeInput('pwd\r')
    expect(write.mock.calls).toEqual([
      ['\x1b[1;1R'],
      [terminalChangeDirectoryCommand(auxiliary, '/bin/zsh')],
      ['pwd\r']
    ])
    expect(() => input.selectSourceDirectory('main')).toThrow('already received user input')
  })

  it('rejects a late source choice after first key, paste or IME input without writing cd', () => {
    for (const text of ['a', 'paste\n', '中文', '\x03']) {
      const { input, write } = fixture()
      input.writeInput(text)
      expect(() => input.selectSourceDirectory('aux')).toThrow('already received user input')
      expect(write).toHaveBeenCalledExactlyOnceWith(text)
    }
    const { input, write } = fixture()
    input.markUserInput()
    expect(() => input.selectSourceDirectory('aux')).toThrow('already received user input')
    expect(write).not.toHaveBeenCalled()
  })

  it('rejects unknown/cross-project ids and never restores the first-input gate', () => {
    const { input, write } = fixture()
    expect(() => input.selectSourceDirectory('another-project/aux')).toThrow('Unknown')
    expect(() => input.selectSourceDirectory('aux')).toThrow('already received')
    expect(write).not.toHaveBeenCalled()
  })

  it('detects directory replacement and symlink retargeting after opening', () => {
    const { input, write, auxiliary, base } = fixture()
    renameSync(auxiliary, join(base, 'old'))
    mkdirSync(auxiliary)
    expect(() => input.selectSourceDirectory('aux')).toThrow('identity changed')
    expect(write).not.toHaveBeenCalled()
    const f = fixture()
    const link = join(f.base, 'link')
    symlinkSync(f.auxiliary, link)
    f.project.folders[1].path = link
    const redirected = new TerminalSessionInput(
      f.write,
      '/bin/zsh',
      freezeTerminalProjectSources(f.project)
    )
    rmSync(link)
    symlinkSync(f.primary, link)
    expect(() => redirected.selectSourceDirectory('aux')).toThrow('identity changed')
    expect(f.write).not.toHaveBeenCalled()
  })

  it('allows opening primary with an offline auxiliary but never rebinds it after arrival', () => {
    const { project, auxiliary, write } = fixture()
    rmSync(auxiliary, { recursive: true })
    const sources = freezeTerminalProjectSources(project)
    mkdirSync(auxiliary)
    const input = new TerminalSessionInput(write, '/bin/zsh', sources)
    expect(() => input.selectSourceDirectory('aux')).toThrow('was unavailable')
    expect(write).not.toHaveBeenCalled()
  })

  it('executes only cd for shell metacharacters, spaces, quotes and unicode', () => {
    const { auxiliary, primary } = fixture()
    for (const shell of ['/bin/sh', '/bin/bash', '/bin/zsh']) {
      const command = terminalChangeDirectoryCommand(auxiliary, shell).replace(/\r$/, '\n')
      const result = spawnSync(shell, ['-c', `${command}pwd`], { cwd: primary, encoding: 'utf8' })
      expect(result.status, result.stderr).toBe(0)
      expect(result.stdout.trim()).toBe(auxiliary)
    }
  })

  it('rejects terminal control bytes and unsupported shells; quotes PowerShell literally', () => {
    for (const path of ['/tmp/a\nb', '/tmp/a\rb', '/tmp/a\x1bb', '/tmp/a\x00b']) {
      expect(() => terminalChangeDirectoryCommand(path, '/bin/zsh')).toThrow('control characters')
    }
    expect(() => terminalChangeDirectoryCommand('/tmp/a', '/usr/bin/custom')).toThrow(
      'not supported'
    )
    expect(terminalChangeDirectoryCommand("C:\\a'b$()", 'powershell.exe')).toBe(
      "Set-Location -LiteralPath 'C:\\a''b$()'\r"
    )
  })
})
