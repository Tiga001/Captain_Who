import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { NotificationLocaleStore } from '../notifications/notificationLocaleStore'

const roots: string[] = []

async function temporaryDirectory(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'mycopilot-notification-locale-'))
  roots.push(root)
  return root
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })))
})

describe('NotificationLocaleStore', () => {
  it('defaults to zh-CN and restores a strictly persisted application language', async () => {
    const root = await temporaryDirectory()
    const first = new NotificationLocaleStore(root)
    expect(first.getLocale()).toBe('zh-CN')

    await first.setLocale('en-GB')
    const restarted = new NotificationLocaleStore(root)
    expect(restarted.getLocale()).toBe('en-GB')
  })

  it('rejects invalid IPC values and fail-closes malformed or extended files', async () => {
    const root = await temporaryDirectory()
    const store = new NotificationLocaleStore(root)
    await expect(store.setLocale('not-a-language')).rejects.toThrow('Invalid application language')
    expect(store.getLocale()).toBe('zh-CN')

    await writeFile(
      join(root, 'notification-locale-v1.json'),
      JSON.stringify({ schemaVersion: 1, language: 'en-US', injected: true })
    )
    expect(new NotificationLocaleStore(root).getLocale()).toBe('zh-CN')
  })

  it('does not follow a locale symlink or read an oversized mirror', async () => {
    const parent = await temporaryDirectory()
    const target = join(parent, 'outside.json')
    await writeFile(target, JSON.stringify({ schemaVersion: 1, language: 'ru-RU' }))
    const symlinkRoot = join(parent, 'symlink-root')
    await mkdir(symlinkRoot)
    await symlink(target, join(symlinkRoot, 'notification-locale-v1.json'))
    expect(new NotificationLocaleStore(symlinkRoot).getLocale()).toBe('zh-CN')

    const oversizedRoot = join(parent, 'oversized-root')
    await mkdir(oversizedRoot)
    await writeFile(join(oversizedRoot, 'notification-locale-v1.json'), 'x'.repeat(4_097))
    expect(new NotificationLocaleStore(oversizedRoot).getLocale()).toBe('zh-CN')
  })

  it('switches the current session immediately and never lets an older write roll it back', async () => {
    const root = await temporaryDirectory()
    const store = new NotificationLocaleStore(root)
    const firstWrite = store.setLocale('zh-TW')
    const secondWrite = store.setLocale('ja-JP')

    expect(store.getLocale()).toBe('ja-JP')
    await firstWrite
    expect(store.getLocale()).toBe('ja-JP')
    await secondWrite
    expect(new NotificationLocaleStore(root).getLocale()).toBe('ja-JP')
  })

  it('keeps the immediate session language after persistence failure and can recover later', async () => {
    const parent = await temporaryDirectory()
    const blockedRoot = join(parent, 'user-data')
    await writeFile(blockedRoot, 'not a directory')
    const store = new NotificationLocaleStore(blockedRoot)

    await expect(store.setLocale('fr-FR')).rejects.toThrow()
    expect(store.getLocale()).toBe('fr-FR')

    await rm(blockedRoot)
    await mkdir(blockedRoot)
    await store.setLocale('it-IT')
    expect(new NotificationLocaleStore(blockedRoot).getLocale()).toBe('it-IT')
  })

  it('waits for accepted writes at shutdown and rejects new writes after the fence', async () => {
    const root = await temporaryDirectory()
    const store = new NotificationLocaleStore(root)
    const accepted = store.setLocale('ko-KR')
    const shutdown = store.beginShutdown()

    await Promise.all([accepted, shutdown])
    expect(new NotificationLocaleStore(root).getLocale()).toBe('ko-KR')
    await expect(store.setLocale('en-US')).rejects.toThrow(
      'Notification locale store is shutting down'
    )
  })
})
