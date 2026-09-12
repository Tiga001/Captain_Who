import { mkdtemp, mkdir, readFile, realpath, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { StorageProjectRecord } from '@mycopilot/protocol'
import { resolveProjectFileReference } from './projectFilePaths'

let directory: string
let primary: string
let auxiliary: string
let project: StorageProjectRecord
const loadProjects = vi.fn(async () => [project])
const resolveRunWorkspacePath =
  vi.fn<
    (input: {
      assistantMessageId: string
      projectId?: string | null
      filePath: string
    }) => Promise<string>
  >()
const source = { loadProjects, resolveRunWorkspacePath }

beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'captain-project-paths-'))
  primary = join(directory, 'app')
  auxiliary = join(directory, 'docs')
  await Promise.all([mkdir(primary), mkdir(auxiliary)])
  await Promise.all([
    writeFile(join(primary, 'same.txt'), 'primary'),
    writeFile(join(auxiliary, 'same.txt'), 'auxiliary')
  ])
  project = {
    id: 'project',
    name: 'Workspace',
    createdAt: 1,
    pinnedAt: null,
    folders: [
      {
        id: 'app-folder',
        alias: 'app',
        role: 'primary',
        path: primary,
        sortOrder: 0,
        createdAt: 1
      },
      {
        id: 'docs-folder',
        alias: 'docs',
        role: 'auxiliary',
        path: auxiliary,
        sortOrder: 1,
        createdAt: 1
      }
    ]
  }
  loadProjects.mockClear()
  resolveRunWorkspacePath.mockReset()
})

afterEach(async () => {
  await rm(directory, { recursive: true, force: true })
})

const current = (filePath: string) =>
  resolveProjectFileReference(source, { projectId: 'project', filePath })
const historical = (filePath: string) =>
  resolveProjectFileReference(source, {
    projectId: 'project',
    filePath,
    assistantMessageId: 'assistant'
  })

it('resolves default primary and explicit auxiliary paths without mixing duplicate names', async () => {
  expect(await readFile(await current('same.txt'), 'utf8')).toBe('primary')
  expect(await readFile(await current('@workspace/docs/same.txt'), 'utf8')).toBe('auxiliary')
  expect(await current('@workspace/docs')).toBe(await realpath(auxiliary))
})

it('routes historical references to Rust without consulting the current project configuration', async () => {
  const frozenPath = join(primary, 'same.txt')
  resolveRunWorkspacePath.mockResolvedValue(frozenPath)
  project.folders = []
  expect(await historical('@workspace/docs/same.txt')).toBe(frozenPath)
  expect(loadProjects).not.toHaveBeenCalled()
  expect(resolveRunWorkspacePath).toHaveBeenCalledWith({
    assistantMessageId: 'assistant',
    projectId: 'project',
    filePath: '@workspace/docs/same.txt'
  })
})

it('does not fall back when Rust rejects a missing or changed historical root', async () => {
  resolveRunWorkspacePath.mockRejectedValue(new Error('Frozen root no longer available'))
  await expect(historical('same.txt')).rejects.toThrow('no longer available')
  await expect(historical('/old/absolute/path.png')).rejects.toThrow('no longer available')
  expect(loadProjects).not.toHaveBeenCalled()
})

it('rejects unknown aliases, unknown virtual prefixes, traversal and symlink escapes', async () => {
  await symlink(auxiliary, join(primary, 'outside'))
  for (const path of [
    '@workspace/missing/same.txt',
    '@unknown/same.txt',
    '@workspace/docs/../app/same.txt',
    '../docs/same.txt',
    'outside/same.txt'
  ]) {
    await expect(current(path)).rejects.toThrow()
  }
})

it('keeps the literal ./@workspace directory distinct from the virtual prefix', async () => {
  await mkdir(join(primary, '@workspace', 'docs'), { recursive: true })
  await writeFile(join(primary, '@workspace', 'docs', 'same.txt'), 'literal')
  expect(await readFile(await current('./@workspace/docs/same.txt'), 'utf8')).toBe('literal')
})
