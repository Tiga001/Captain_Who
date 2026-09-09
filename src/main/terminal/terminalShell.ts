import { posix } from 'node:path'

export function getTerminalShellLaunch(
  platform: NodeJS.Platform,
  configuredShell?: string
): { shell: string; args: string[] } {
  if (platform === 'win32') {
    return { shell: 'powershell.exe', args: [] }
  }

  const shell = configuredShell || (platform === 'darwin' ? '/bin/zsh' : '/bin/sh')
  const shellName = posix.basename(shell)
  // GUI-launched macOS apps may lack PATH entries initialized by login profiles.
  // Only pass the login flag to shells whose startup contract we support.
  const useLoginShell = platform === 'darwin' && (shellName === 'zsh' || shellName === 'bash')

  return { shell, args: useLoginShell ? ['-l'] : [] }
}
