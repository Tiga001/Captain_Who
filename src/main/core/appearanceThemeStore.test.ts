import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import {
  AppearanceThemeStore,
  resolveAppearanceColorScheme
} from '../appearance/appearanceThemeStore'

const roots: string[] = []

async function temporaryDirectory(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mycopilot-appearance-theme-'))
  roots.push(root)
  return root
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })))
})

describe('resolveAppearanceColorScheme', () => {
  it('honors an explicit app preference over the system scheme', () => {
    expect(resolveAppearanceColorScheme('dark', false)).toBe('dark')
    expect(resolveAppearanceColorScheme('light', true)).toBe('light')
    expect(resolveAppearanceColorScheme('system', true)).toBe('dark')
    expect(resolveAppearanceColorScheme('system', false)).toBe('light')
  })
})

describe('AppearanceThemeStore', () => {
  it('defaults to system and restores a strictly persisted appearance preference', async () => {
    const root = await temporaryDirectory()
    const first = new AppearanceThemeStore(root)
    expect(first.getPreference()).toBe('system')

    await first.setPreference('dark')
    const restarted = new AppearanceThemeStore(root)
    expect(restarted.getPreference()).toBe('dark')
  })

  it('rejects invalid IPC values and fail-closes malformed or extended files', async () => {
    const root = await temporaryDirectory()
    const store = new AppearanceThemeStore(root)
    await expect(store.setPreference('sepia')).rejects.toThrow('Invalid native theme source')
    expect(store.getPreference()).toBe('system')

    await writeFile(
      join(root, 'appearance-v1.json'),
      JSON.stringify({ schemaVersion: 1, colorSchemePreference: 'dark', injected: true })
    )
    expect(new AppearanceThemeStore(root).getPreference()).toBe('system')
  })

  it('does not follow an appearance symlink or read an oversized mirror', async () => {
    const parent = await temporaryDirectory()
    const target = join(parent, 'outside.json')
    await writeFile(target, JSON.stringify({ schemaVersion: 1, colorSchemePreference: 'dark' }))
    const symlinkRoot = join(parent, 'symlink-root')
    await mkdir(symlinkRoot)
    await symlink(target, join(symlinkRoot, 'appearance-v1.json'))
    expect(new AppearanceThemeStore(symlinkRoot).getPreference()).toBe('system')

    const oversizedRoot = join(parent, 'oversized-root')
    await mkdir(oversizedRoot)
    await writeFile(join(oversizedRoot, 'appearance-v1.json'), 'x'.repeat(4_097))
    expect(new AppearanceThemeStore(oversizedRoot).getPreference()).toBe('system')
  })

  it('switches the current session immediately and never lets an older write roll it back', async () => {
    const root = await temporaryDirectory()
    const store = new AppearanceThemeStore(root)
    const firstWrite = store.setPreference('light')
    const secondWrite = store.setPreference('dark')

    expect(store.getPreference()).toBe('dark')
    await firstWrite
    expect(store.getPreference()).toBe('dark')
    await secondWrite
    expect(new AppearanceThemeStore(root).getPreference()).toBe('dark')
  })

  it('keeps the immediate session preference after persistence failure and can recover later', async () => {
    const parent = await temporaryDirectory()
    const blockedRoot = join(parent, 'user-data')
    await writeFile(blockedRoot, 'not a directory')
    const store = new AppearanceThemeStore(blockedRoot)

    await expect(store.setPreference('dark')).rejects.toThrow()
    expect(store.getPreference()).toBe('dark')

    await rm(blockedRoot)
    await mkdir(blockedRoot)
    await store.setPreference('light')
    expect(new AppearanceThemeStore(blockedRoot).getPreference()).toBe('light')
  })

  it('waits for accepted writes at shutdown and rejects new writes after the fence', async () => {
    const root = await temporaryDirectory()
    const store = new AppearanceThemeStore(root)
    const accepted = store.setPreference('dark')
    const shutdown = store.beginShutdown()

    await Promise.all([accepted, shutdown])
    expect(new AppearanceThemeStore(root).getPreference()).toBe('dark')
    await expect(store.setPreference('light')).rejects.toThrow(
      'Appearance theme store is shutting down'
    )
  })
})
