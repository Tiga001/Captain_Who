import { realpathSync, statSync } from 'node:fs'
import { isAbsolute, basename } from 'node:path'
import type { StorageProjectRecord, TerminalSourceFolder } from '@mycopilot/protocol'

export interface TerminalSourceDirectory extends TerminalSourceFolder {
  identity?: { canonicalPath: string; device: string; inode: string }
}

export interface TerminalProjectSources {
  projectId: string
  primaryFolderId: string
  folders: TerminalSourceDirectory[]
}

function directoryIdentity(path: string): NonNullable<TerminalSourceDirectory['identity']> {
  if (!isAbsolute(path)) throw new Error('Terminal source directory must be absolute')
  const canonicalPath = realpathSync(path)
  const info = statSync(canonicalPath, { bigint: true })
  if (!info.isDirectory()) throw new Error('Terminal source is not a directory')
  return { canonicalPath, device: String(info.dev), inode: String(info.ino) }
}

/** Only Main's stored project record supplies paths; the renderer selects by folder id. */
export function freezeTerminalProjectSources(
  project: StorageProjectRecord
): TerminalProjectSources | undefined {
  if (project.folders.length === 0) return undefined
  const primary = project.folders.filter((folder) => folder.role === 'primary')
  if (
    primary.length !== 1 ||
    new Set(project.folders.map((folder) => folder.id)).size !== project.folders.length
  ) {
    throw new Error('Invalid terminal project source directories')
  }
  return {
    projectId: project.id,
    primaryFolderId: primary[0].id,
    folders: project.folders.map((folder) => {
      const display: TerminalSourceFolder = {
        id: folder.id,
        alias: folder.alias,
        path: folder.path,
        role: folder.role
      }
      try {
        return { ...display, identity: directoryIdentity(folder.path) }
      } catch (error) {
        if (folder.id === primary[0].id) throw error
        // An offline auxiliary source must not prevent opening the primary terminal.
        return display
      }
    })
  }
}

export function validateTerminalSourceDirectory(folder: TerminalSourceDirectory): string {
  const expected = folder.identity
  if (!expected)
    throw new Error('Terminal source directory was unavailable when the terminal opened')
  const current = directoryIdentity(folder.path)
  if (
    current.canonicalPath !== expected.canonicalPath ||
    current.device !== expected.device ||
    current.inode !== expected.inode
  ) {
    throw new Error('Terminal source directory identity changed; open a new terminal')
  }
  return current.canonicalPath
}

export function terminalChangeDirectoryCommand(path: string, shell: string): string {
  // Control bytes are interpreted by the terminal line editor even inside shell quotes.
  if (
    Array.from(path).some(
      (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127
    )
  )
    throw new Error('Terminal source directory contains control characters')
  const shellName = basename(shell).toLowerCase()
  if (shellName === 'powershell.exe' || shellName === 'pwsh.exe' || shellName === 'pwsh') {
    return `Set-Location -LiteralPath '${path.replaceAll("'", "''")}'\r`
  }
  if (!['sh', 'bash', 'zsh', 'dash', 'ksh', 'fish', 'ash'].includes(shellName)) {
    throw new Error('Directory selection is not supported by this terminal shell')
  }
  return `cd -- '${path.replaceAll("'", "'\"'\"'")}'\r`
}

/** The utility event loop serializes this gate with normal input on the same PTY. */
export class TerminalSessionInput {
  private hasUserInput = false

  constructor(
    private readonly write: (data: string) => void,
    private readonly shell: string,
    private readonly sources?: TerminalProjectSources
  ) {}

  markUserInput(): void {
    this.hasUserInput = true
  }

  writeInput(data: string, userInitiated = true): void {
    if (!data) return
    if (userInitiated) this.markUserInput()
    this.write(data)
  }

  selectSourceDirectory(folderId: string): void {
    if (this.hasUserInput) throw new Error('Terminal has already received user input')
    // A selection attempt itself is the first input, including a failed attempt.
    this.markUserInput()
    const folder = this.sources?.folders.find((entry) => entry.id === folderId)
    if (!folder || this.sources!.folders.length < 2)
      throw new Error('Unknown terminal source directory')
    const path = validateTerminalSourceDirectory(folder)
    this.write(terminalChangeDirectoryCommand(path, this.shell))
  }
}
