import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { getTerminalShellLaunch } from './terminalShell'

describe('getTerminalShellLaunch', () => {
  it.each([undefined, ''])('defaults to a login zsh on macOS when SHELL is %s', (shell) => {
    expect(getTerminalShellLaunch('darwin', shell)).toEqual({
      shell: '/bin/zsh',
      args: ['-l']
    })
  })

  it.each(['/bin/zsh', '/bin/bash', '/opt/homebrew/bin/zsh', '/usr/local/bin/bash'])(
    'starts the configured macOS shell %s as a login shell',
    (shell) => {
      expect(getTerminalShellLaunch('darwin', shell)).toEqual({ shell, args: ['-l'] })
    }
  )

  it.each(['/opt/homebrew/bin/fish', '/bin/sh', '/custom/bin/my-zsh', '/custom/zsh/wrapper'])(
    'preserves the arguments of other macOS shells such as %s',
    (shell) => {
      expect(getTerminalShellLaunch('darwin', shell)).toEqual({ shell, args: [] })
    }
  )

  it.each([undefined, '', '/bin/zsh', 'C:\\custom\\bash.exe'])(
    'always selects PowerShell on Windows regardless of SHELL=%s',
    (shell) => {
      expect(getTerminalShellLaunch('win32', shell)).toEqual({
        shell: 'powershell.exe',
        args: []
      })
    }
  )

  it.each(['linux', 'freebsd'] as const)('preserves non-login shell launches on %s', (platform) => {
    expect(getTerminalShellLaunch(platform)).toEqual({ shell: '/bin/sh', args: [] })
    expect(getTerminalShellLaunch(platform, '')).toEqual({ shell: '/bin/sh', args: [] })
    expect(getTerminalShellLaunch(platform, '/custom/bin/zsh')).toEqual({
      shell: '/custom/bin/zsh',
      args: []
    })
    expect(getTerminalShellLaunch(platform, '/custom/bin/bash')).toEqual({
      shell: '/custom/bin/bash',
      args: []
    })
  })
})

describe.skipIf(process.platform !== 'darwin')('macOS interactive shell startup', () => {
  let root: string
  let cwd: string
  let env: NodeJS.ProcessEnv
  let tracePath: string

  beforeEach(() => {
    root = realpathSync(mkdtempSync(join(tmpdir(), 'terminal-login-shell-')))
    const home = join(root, 'home')
    const initialBin = join(root, 'empty-bin')
    const brewBin = join(home, 'fake-homebrew', 'bin')
    cwd = join(root, 'project with spaces')
    tracePath = join(root, 'startup-trace')
    for (const directory of [home, initialBin, brewBin, cwd]) {
      mkdirSync(directory, { recursive: true })
    }

    // Do not inherit the host's PATH, shell environment, or user startup files.
    env = {
      HOME: home,
      ZDOTDIR: home,
      PATH: initialBin,
      LC_ALL: 'C',
      TERM: 'dumb',
      TERMINAL_TEST_TRACE: tracePath
    }
    // Disable global startup files after zsh's mandatory /etc/zshenv phase.
    writeFileSync(join(home, '.zshenv'), 'unsetopt GLOBAL_RCS\n')
    writeFileSync(
      join(home, '.zprofile'),
      'printf "zprofile\\n" >> "$TERMINAL_TEST_TRACE"\n' +
        'export PATH="$HOME/fake-homebrew/bin:$PATH"\n'
    )
    writeFileSync(
      join(home, '.zshrc'),
      'printf "zshrc\\n" >> "$TERMINAL_TEST_TRACE"\n' + 'eval "$(brew shellenv)"\n'
    )
    writeFileSync(
      join(brewBin, 'brew'),
      '#!/bin/sh\n' +
        '[ "$1" = shellenv ] || exit 64\n' +
        'printf "brew\\n" >> "$TERMINAL_TEST_TRACE"\n' +
        'printf "export TERMINAL_TEST_BREW_RESULT=fake-brew-ready\\n"\n',
      { mode: 0o755 }
    )
  })

  afterEach(() => {
    if (root) rmSync(root, { recursive: true, force: true })
  })

  function runInteractiveShell(args: string[]): ReturnType<typeof spawnSync> {
    return spawnSync(
      '/bin/zsh',
      [
        ...args,
        '-i',
        '-c',
        'printf "brew=%s\\ncwd=%s\\n" "$TERMINAL_TEST_BREW_RESULT" "$PWD"; exit 37'
      ],
      { cwd, env, encoding: 'utf8', timeout: 5_000 }
    )
  }

  it('loads the login PATH before .zshrc and preserves the working directory and exit code', () => {
    const launch = getTerminalShellLaunch('darwin', '/bin/zsh')
    const result = runInteractiveShell(launch.args)

    expect(result.error).toBeUndefined()
    expect(result.signal).toBeNull()
    expect(result.status).toBe(37)
    expect(result.stderr).toBe('')
    expect(result.stdout).toBe(`brew=fake-brew-ready\ncwd=${cwd}\n`)
    expect(readFileSync(tracePath, 'utf8')).toBe('zprofile\nzshrc\nbrew\n')
  })

  it('reproduces the missing brew command when an interactive shell is not a login shell', () => {
    const result = runInteractiveShell([])

    expect(result.error).toBeUndefined()
    expect(result.signal).toBeNull()
    expect(result.status).toBe(37)
    expect(result.stderr).toContain('command not found: brew')
    expect(result.stdout).toBe(`brew=\ncwd=${cwd}\n`)
    expect(readFileSync(tracePath, 'utf8')).toBe('zshrc\n')
  })
})
